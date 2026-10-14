//! OpenAI-compatible streaming chat client (OpenRouter, Ollama, LM Studio, ...) and the
//! prompts fivesight uses to expand notes and answer questions about meetings.

pub mod prompts;
pub mod sse;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("model returned an error ({status}): {body}")]
    Api { status: u16, body: String },
    #[error("stream error: {0}")]
    Stream(String),
}

impl LlmError {
    /// A message for the person using the app, with the likely fix. `key_sent` says whether
    /// the failed request carried an API key.
    pub fn user_message(&self, key_sent: bool) -> String {
        match self {
            LlmError::Api { status: 401 | 403, .. } if !key_sent => {
                "The AI server wants an API key, but none was sent. Add one in Settings, under AI model.".into()
            }
            LlmError::Api { status: 401 | 403, .. } => {
                "The AI server rejected your API key. Check it in Settings, under AI model.".into()
            }
            LlmError::Api { status: 404, .. } => {
                "The AI server doesn't recognise that address or model name. Check both in Settings.".into()
            }
            LlmError::Api { status: 429, .. } => "The AI server is rate limiting you. Try again in a moment.".into(),
            LlmError::Http(e) if e.is_connect() || e.is_timeout() => {
                "Couldn't reach the AI server. Check the address in Settings, or that your local server is running."
                    .into()
            }
            other => other.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// e.g. `https://openrouter.ai/api/v1` or `http://localhost:11434/v1`.
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    /// Tried when the primary model fails before producing any output.
    pub fallback_model: Option<String>,
}

impl LlmConfig {
    pub fn openrouter(api_key: String, model: &str) -> Self {
        Self {
            base_url: "https://openrouter.ai/api/v1".into(),
            api_key: Some(api_key),
            model: model.into(),
            fallback_model: None,
        }
    }

    pub fn local(base_url: &str, model: &str) -> Self {
        Self::custom(base_url, None, model)
    }

    /// Any OpenAI-compatible server. The key is optional (local servers don't need one).
    /// A pasted full endpoint such as `.../v1/chat/completions` is trimmed back to the base.
    pub fn custom(base_url: &str, api_key: Option<String>, model: &str) -> Self {
        let trimmed = base_url.trim().trim_end_matches('/');
        let base = trimmed.strip_suffix("/chat/completions").unwrap_or(trimmed).trim_end_matches('/');
        Self {
            base_url: base.into(),
            api_key: api_key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()),
            model: model.trim().into(),
            fallback_model: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub role: String,
    pub content: String,
    /// Set on an assistant turn that asked for tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Set on a `tool` turn: which call this is the result of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    fn new(role: &str, c: impl Into<String>) -> Self {
        Self { role: role.into(), content: c.into(), ..Default::default() }
    }
    pub fn system(c: impl Into<String>) -> Self {
        Self::new("system", c)
    }
    pub fn user(c: impl Into<String>) -> Self {
        Self::new("user", c)
    }
    pub fn assistant(c: impl Into<String>) -> Self {
        Self::new("assistant", c)
    }
    /// An assistant turn that requested tool calls.
    pub fn assistant_calls(c: impl Into<String>, calls: Vec<ToolCall>) -> Self {
        Self { tool_calls: Some(calls), ..Self::new("assistant", c) }
    }
    /// The result of one tool call.
    pub fn tool(call_id: impl Into<String>, c: impl Into<String>) -> Self {
        Self { tool_call_id: Some(call_id.into()), ..Self::new("tool", c) }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionCall,
}

fn function_kind() -> String {
    "function".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FunctionCall {
    pub name: String,
    /// JSON text, as the model wrote it.
    #[serde(default)]
    pub arguments: String,
}

/// A complete (non-streamed) reply: either text, or a request to call tools.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reply {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

/// One non-streamed request offering `tools` (OpenAI function-calling format).
/// Servers or models without tool support usually answer 400/404/422; callers can then
/// fall back to a plain [`chat_stream`].
pub async fn chat_with_tools(
    cfg: &LlmConfig,
    messages: &[Message],
    tools: &serde_json::Value,
) -> Result<Reply, LlmError> {
    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    let mut req = reqwest::Client::new().post(url).json(&serde_json::json!({
        "model": cfg.model,
        "messages": messages,
        "tools": tools,
        "tool_choice": "auto",
        "stream": false,
    }));
    if let Some(k) = &cfg.api_key {
        req = req.bearer_auth(k);
    }
    let resp = req.send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(LlmError::Api { status: status.as_u16(), body: body.chars().take(400).collect() });
    }
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| LlmError::Stream(format!("unreadable reply: {e}")))?;
    if let Some(err) = v.get("error") {
        return Err(LlmError::Stream(
            err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error").to_string(),
        ));
    }
    let msg = &v["choices"][0]["message"];
    let content = msg["content"].as_str().unwrap_or("").to_string();
    let tool_calls: Vec<ToolCall> =
        msg.get("tool_calls").and_then(|t| serde_json::from_value(t.clone()).ok()).unwrap_or_default();
    Ok(Reply { content, tool_calls })
}

async fn stream_once(
    client: &reqwest::Client,
    cfg: &LlmConfig,
    model: &str,
    messages: &[Message],
    on_delta: &mut (impl FnMut(&str) + Send),
) -> Result<String, LlmError> {
    use futures_util::StreamExt;

    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    let mut req = client.post(url).json(&serde_json::json!({ "model": model, "messages": messages, "stream": true }));
    if let Some(k) = &cfg.api_key {
        req = req.bearer_auth(k);
    }
    let resp = req.send().await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(LlmError::Api { status: status.as_u16(), body: body.chars().take(400).collect() });
    }

    let mut decoder = sse::SseDecoder::default();
    let mut full = String::new();
    let mut bytes = resp.bytes_stream();
    while let Some(chunk) = bytes.next().await {
        for ev in decoder.push(&chunk?) {
            match ev {
                sse::SseEvent::Delta(d) => {
                    on_delta(&d);
                    full.push_str(&d);
                }
                sse::SseEvent::Error(e) => return Err(LlmError::Stream(e)),
                sse::SseEvent::Done => return Ok(full),
            }
        }
    }
    Ok(full)
}

/// Stream a chat completion, calling `on_delta` for each piece of text. Returns the full text.
/// If the primary model fails before any output, the fallback model (if set) is tried once.
pub async fn chat_stream(
    cfg: &LlmConfig,
    messages: &[Message],
    mut on_delta: impl FnMut(&str) + Send,
) -> Result<String, LlmError> {
    let client = reqwest::Client::new();
    let mut produced = false;
    let mut tracked = |d: &str| {
        produced = true;
        on_delta(d);
    };
    match stream_once(&client, cfg, &cfg.model, messages, &mut tracked).await {
        Ok(t) => Ok(t),
        Err(e) if !produced => match &cfg.fallback_model {
            Some(fb) => {
                tracing::warn!("primary model failed ({e}); trying fallback {fb}");
                stream_once(&client, cfg, fb, messages, &mut on_delta).await
            }
            None => Err(e),
        },
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Fake server: answers successive requests with the given (status, body) pairs.
    /// Returns the base URL and a list that records the `model` of each request.
    async fn fake_seq(replies: Vec<(&'static str, String)>) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let models = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = models.clone();
        tokio::spawn(async move {
            for (status, body) in replies {
                let (mut s, _) = l.accept().await.unwrap();
                let mut req = Vec::new();
                let mut buf = vec![0u8; 8192];
                // Read headers, then the JSON body announced by Content-Length.
                loop {
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    req.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&req).to_string();
                    if let Some(h) = text.find("\r\n\r\n") {
                        let len = text[..h]
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if req.len() >= h + 4 + len || n == 0 {
                            let body_json = &text[h + 4..];
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(body_json) {
                                let mut rec = v["model"].as_str().unwrap_or("").to_string();
                                if v.get("tools").is_some() {
                                    rec.push_str("+tools");
                                }
                                seen.lock().unwrap().push(rec);
                            }
                            break;
                        }
                    } else if n == 0 {
                        break;
                    }
                }
                let head = format!("HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
                s.write_all(head.as_bytes()).await.unwrap();
                s.write_all(body.as_bytes()).await.unwrap();
                let _ = s.shutdown().await;
            }
        });
        (format!("http://{addr}"), models)
    }

    async fn fake(status: &'static str, body: String) -> String {
        fake_seq(vec![(status, body)]).await.0
    }

    fn sse(parts: &[&str]) -> String {
        let mut s = String::new();
        for p in parts {
            s.push_str(&format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{p}\"}}}}]}}\n\n"));
        }
        s.push_str("data: [DONE]\n\n");
        s
    }

    #[test]
    fn user_messages_explain_the_fix() {
        let api = |status| LlmError::Api {
            status,
            body: "{\"error\":{\"message\":\"Missing Authentication header\"}}".into(),
        };
        assert!(api(401).user_message(false).contains("none was sent"));
        assert!(api(401).user_message(true).contains("rejected your API key"));
        assert!(api(404).user_message(true).contains("model name"));
        assert!(api(429).user_message(true).contains("rate limiting"));
        // Unknown statuses keep the raw detail so nothing is hidden.
        assert!(api(500).user_message(true).contains("500"));
        assert!(LlmError::Stream("cut".into()).user_message(true).contains("cut"));
    }

    #[test]
    fn custom_normalizes_url_and_key() {
        let c = LlmConfig::custom(" https://api.example.com/v1/chat/completions/ ", Some("  sk-1 ".into()), " m ");
        assert_eq!(c.base_url, "https://api.example.com/v1");
        assert_eq!(c.api_key.as_deref(), Some("sk-1"));
        assert_eq!(c.model, "m");
        assert_eq!(LlmConfig::custom("http://localhost:11434/v1/", Some("   ".into()), "x").api_key, None);
    }

    /// Serve one request and hand back the raw request head the client sent.
    async fn capture_head() -> (String, tokio::sync::oneshot::Receiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = s.read(&mut buf).await.unwrap_or(0);
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase());
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\ndata: [DONE]\n\n").await;
            let _ = s.shutdown().await;
        });
        (format!("http://{addr}"), rx)
    }

    #[tokio::test]
    async fn sends_bearer_key_when_configured() {
        let (url, head) = capture_head().await;
        let cfg = LlmConfig::custom(&url, Some("sk-test".into()), "m");
        chat_stream(&cfg, &[Message::user("x")], |_| {}).await.unwrap();
        assert!(head.await.unwrap().contains("authorization: bearer sk-test"));
    }

    #[tokio::test]
    async fn sends_no_auth_header_without_a_key() {
        let (url, head) = capture_head().await;
        chat_stream(&LlmConfig::custom(&url, None, "m"), &[Message::user("x")], |_| {}).await.unwrap();
        assert!(!head.await.unwrap().contains("authorization"));
    }

    #[tokio::test]
    async fn streams_deltas_in_order() {
        let url = fake("200 OK", sse(&["Hel", "lo ", "world"])).await;
        let cfg = LlmConfig::local(&url, "m");
        let mut seen = Vec::new();
        let full = chat_stream(&cfg, &[Message::user("hi")], |d| seen.push(d.to_string())).await.unwrap();
        assert_eq!(full, "Hello world");
        assert_eq!(seen, ["Hel", "lo ", "world"]);
    }

    #[tokio::test]
    async fn tool_calls_are_requested_and_parsed() {
        let reply = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
            {"id":"call_1","type":"function","function":{"name":"jira_view","arguments":"{\"key\":\"PAY-142\"}"}}]}}]}"#;
        let (url, seen) = fake_seq(vec![("200 OK", reply.into())]).await;
        let cfg = LlmConfig::local(&url, "m");
        let tools = serde_json::json!([{"type": "function", "function": {"name": "jira_view", "parameters": {}}}]);
        let r = chat_with_tools(&cfg, &[Message::user("what is PAY-142?")], &tools).await.unwrap();
        assert_eq!(r.content, "");
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].function.name, "jira_view");
        assert_eq!(r.tool_calls[0].function.arguments, r#"{"key":"PAY-142"}"#);
        assert_eq!(seen.lock().unwrap().as_slice(), ["m+tools"]);
    }

    #[tokio::test]
    async fn a_plain_answer_comes_back_as_content() {
        let reply = r#"{"choices":[{"message":{"role":"assistant","content":"No lookup needed."}}]}"#;
        let url = fake("200 OK", reply.into()).await;
        let r = chat_with_tools(&LlmConfig::local(&url, "m"), &[Message::user("hi")], &serde_json::json!([]))
            .await
            .unwrap();
        assert_eq!(r, Reply { content: "No lookup needed.".into(), tool_calls: vec![] });
    }

    #[tokio::test]
    async fn tool_support_errors_surface_for_fallback() {
        let url = fake("400 Bad Request", r#"{"error":{"message":"tools not supported"}}"#.into()).await;
        let err = chat_with_tools(&LlmConfig::local(&url, "m"), &[Message::user("hi")], &serde_json::json!([]))
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Api { status: 400, .. }), "{err}");
    }

    #[test]
    fn tool_turns_serialize_in_openai_shape() {
        let call = ToolCall {
            id: "c1".into(),
            kind: "function".into(),
            function: FunctionCall { name: "jira_view".into(), arguments: "{}".into() },
        };
        let a = serde_json::to_value(Message::assistant_calls("", vec![call])).unwrap();
        assert_eq!(a["tool_calls"][0]["type"], "function");
        assert_eq!(a["tool_calls"][0]["function"]["name"], "jira_view");
        let t = serde_json::to_value(Message::tool("c1", "result")).unwrap();
        assert_eq!(t["role"], "tool");
        assert_eq!(t["tool_call_id"], "c1");
        // Plain turns stay exactly {role, content}.
        assert_eq!(
            serde_json::to_value(Message::user("hi")).unwrap(),
            serde_json::json!({"role": "user", "content": "hi"})
        );
    }

    #[tokio::test]
    async fn surfaces_api_errors() {
        let url = fake("401 Unauthorized", "{\"error\":\"bad key\"}".into()).await;
        let cfg = LlmConfig::local(&url, "m");
        let err = chat_stream(&cfg, &[Message::user("hi")], |_| {}).await.unwrap_err();
        assert!(matches!(err, LlmError::Api { status: 401, .. }), "{err}");
    }

    #[tokio::test]
    async fn falls_back_when_primary_fails_before_output() {
        let (url, models) =
            fake_seq(vec![("500 Internal Server Error", "boom".into()), ("200 OK", sse(&["from fallback"]))]).await;
        let mut cfg = LlmConfig::local(&url, "primary");
        cfg.fallback_model = Some("fallback".into());
        let out = chat_stream(&cfg, &[Message::user("x")], |_| {}).await.unwrap();
        assert_eq!(out, "from fallback");
        assert_eq!(*models.lock().unwrap(), ["primary", "fallback"]);
    }

    #[tokio::test]
    async fn no_fallback_once_output_has_started() {
        // Stream emits text, then a mid-stream error: must not retry on the fallback model.
        let body = format!(
            "{}data: {{\"error\":{{\"message\":\"cut\"}}}}\n\n",
            sse(&["partial"]).replace("data: [DONE]\n\n", "")
        );
        let (url, models) = fake_seq(vec![("200 OK", body), ("200 OK", sse(&["should not appear"]))]).await;
        let mut cfg = LlmConfig::local(&url, "primary");
        cfg.fallback_model = Some("fallback".into());
        let err = chat_stream(&cfg, &[Message::user("x")], |_| {}).await.unwrap_err();
        assert!(matches!(err, LlmError::Stream(_)), "{err}");
        assert_eq!(*models.lock().unwrap(), ["primary"]);
    }
}
