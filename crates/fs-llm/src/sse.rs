//! Incremental decoder for OpenAI-style server-sent events.
//!
//! Network chunks can split a line, or even a multi-byte character, anywhere, so bytes are
//! buffered until a full line is available.

#[derive(Debug, PartialEq, Eq)]
pub enum SseEvent {
    Delta(String),
    /// A piece of a tool call. The id and name come once; `arguments` arrive bit by bit.
    ToolCall {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: String,
    },
    /// What the request cost, sent near the end when asked for.
    Usage {
        prompt_tokens: u64,
        cached_tokens: u64,
    },
    Error(String),
    Done,
}

#[derive(Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
}

impl SseDecoder {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line);
            parse_line(line.trim_end_matches(['\r', '\n']), &mut out);
        }
        out
    }
}

fn parse_line(line: &str, out: &mut Vec<SseEvent>) {
    // Comment/keep-alive lines (": OPENROUTER PROCESSING") and blank separators carry nothing.
    let Some(data) = line.strip_prefix("data:").map(str::trim) else { return };
    if data == "[DONE]" {
        out.push(SseEvent::Done);
        return;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else { return };
    if let Some(err) = v.get("error") {
        let msg = err.get("message").and_then(|m| m.as_str()).or_else(|| err.as_str()).unwrap_or("unknown error");
        out.push(SseEvent::Error(msg.to_string()));
        return;
    }
    if let Some(text) = v.pointer("/choices/0/delta/content").and_then(|t| t.as_str()) {
        if !text.is_empty() {
            out.push(SseEvent::Delta(text.to_string()));
        }
    }
    let calls = v.pointer("/choices/0/delta/tool_calls").and_then(|t| t.as_array());
    for (i, call) in calls.into_iter().flatten().enumerate() {
        let text = |path: &str| call.pointer(path).and_then(|t| t.as_str()).filter(|t| !t.is_empty()).map(String::from);
        out.push(SseEvent::ToolCall {
            index: call.get("index").and_then(|n| n.as_u64()).map_or(i, |n| n as usize),
            id: text("/id"),
            name: text("/function/name"),
            arguments: text("/function/arguments").unwrap_or_default(),
        });
    }
    if let Some(prompt_tokens) = v.pointer("/usage/prompt_tokens").and_then(|n| n.as_u64()) {
        // OpenAI and OpenRouter report it one way, DeepSeek's own API the other.
        let cached_tokens = v
            .pointer("/usage/prompt_tokens_details/cached_tokens")
            .or_else(|| v.pointer("/usage/prompt_cache_hit_tokens"))
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        out.push(SseEvent::Usage { prompt_tokens, cached_tokens });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(s: &str) -> String {
        format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{s}\"}}}}]}}\n\n")
    }

    #[test]
    fn parses_deltas_and_done() {
        let mut d = SseDecoder::default();
        let input = format!("{}{}data: [DONE]\n\n", delta("a"), delta("b"));
        assert_eq!(
            d.push(input.as_bytes()),
            [SseEvent::Delta("a".into()), SseEvent::Delta("b".into()), SseEvent::Done]
        );
    }

    #[test]
    fn handles_lines_split_across_chunks() {
        let mut d = SseDecoder::default();
        let full = delta("hello");
        let (a, b) = full.as_bytes().split_at(17);
        assert!(d.push(a).is_empty());
        assert_eq!(d.push(b), [SseEvent::Delta("hello".into())]);
    }

    #[test]
    fn handles_multibyte_char_split_across_chunks() {
        let mut d = SseDecoder::default();
        let full = delta("é世");
        let bytes = full.as_bytes();
        let cut = bytes.iter().position(|&b| b == 0xC3).unwrap() + 1; // between the two bytes of é
        assert!(d.push(&bytes[..cut]).is_empty());
        assert_eq!(d.push(&bytes[cut..]), [SseEvent::Delta("é世".into())]);
    }

    #[test]
    fn ignores_keepalive_comments_and_empty_deltas() {
        let mut d = SseDecoder::default();
        let input = ": OPENROUTER PROCESSING\n\ndata: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n";
        assert!(d.push(input.as_bytes()).is_empty());
    }

    #[test]
    fn surfaces_midstream_errors() {
        let mut d = SseDecoder::default();
        let ev = d.push(b"data: {\"error\":{\"message\":\"rate limited\"}}\n\n");
        assert_eq!(ev, [SseEvent::Error("rate limited".into())]);
    }

    #[test]
    fn collects_tool_call_pieces() {
        let mut d = SseDecoder::default();
        let first = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"jira_view","arguments":""}}]}}]}"#;
        let next = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"key\""}}]}}]}"#;
        let ev = d.push(format!("{first}\n\n{next}\n\n").as_bytes());
        assert_eq!(
            ev,
            [
                SseEvent::ToolCall {
                    index: 0,
                    id: Some("c1".into()),
                    name: Some("jira_view".into()),
                    arguments: String::new()
                },
                SseEvent::ToolCall { index: 0, id: None, name: None, arguments: "{\"key\"".into() },
            ]
        );
    }

    #[test]
    fn reads_cached_tokens_however_the_server_reports_them() {
        let mut d = SseDecoder::default();
        let openai =
            r#"data: {"choices":[],"usage":{"prompt_tokens":100,"prompt_tokens_details":{"cached_tokens":80}}}"#;
        let deepseek = r#"data: {"choices":[],"usage":{"prompt_tokens":50,"prompt_cache_hit_tokens":40}}"#;
        let plain = r#"data: {"choices":[],"usage":{"prompt_tokens":10}}"#;
        let empty = r#"data: {"choices":[],"usage":null}"#;
        let ev = d.push(format!("{openai}\n{deepseek}\n{plain}\n{empty}\n").as_bytes());
        assert_eq!(
            ev,
            [
                SseEvent::Usage { prompt_tokens: 100, cached_tokens: 80 },
                SseEvent::Usage { prompt_tokens: 50, cached_tokens: 40 },
                SseEvent::Usage { prompt_tokens: 10, cached_tokens: 0 },
            ]
        );
    }

    #[test]
    fn handles_crlf() {
        let mut d = SseDecoder::default();
        let ev = d.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\r\n\r\n");
        assert_eq!(ev, [SseEvent::Delta("x".into())]);
    }
}
