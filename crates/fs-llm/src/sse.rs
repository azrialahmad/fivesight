//! Incremental decoder for OpenAI-style server-sent events.
//!
//! Network chunks can split a line, or even a multi-byte character, anywhere, so bytes are
//! buffered until a full line is available.

#[derive(Debug, PartialEq, Eq)]
pub enum SseEvent {
    Delta(String),
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
            if let Some(ev) = parse_line(line.trim_end_matches(['\r', '\n'])) {
                out.push(ev);
            }
        }
        out
    }
}

fn parse_line(line: &str) -> Option<SseEvent> {
    // Comment/keep-alive lines (": OPENROUTER PROCESSING") and blank separators carry nothing.
    let data = line.strip_prefix("data:")?.trim();
    if data == "[DONE]" {
        return Some(SseEvent::Done);
    }
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    if let Some(err) = v.get("error") {
        let msg = err.get("message").and_then(|m| m.as_str()).or_else(|| err.as_str()).unwrap_or("unknown error");
        return Some(SseEvent::Error(msg.to_string()));
    }
    let text = v.pointer("/choices/0/delta/content")?.as_str()?;
    (!text.is_empty()).then(|| SseEvent::Delta(text.to_string()))
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
    fn handles_crlf() {
        let mut d = SseDecoder::default();
        let ev = d.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\r\n\r\n");
        assert_eq!(ev, [SseEvent::Delta("x".into())]);
    }
}
