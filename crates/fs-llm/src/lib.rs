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
        Self { base_url: base_url.trim_end_matches('/').into(), api_key: None, model: model.into(), fallback_model: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn system(c: impl Into<String>) -> Self {
        Self { role: "system".into(), content: c.into() }
    }
    pub fn user(c: impl Into<String>) -> Self {
        Self { role: "user".into(), content: c.into() }
    }
    pub fn assistant(c: impl Into<String>) -> Self {
        Self { role: "assistant".into(), content: c.into() }
    }
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
    let mut req = client
        .post(url)
        .json(&serde_json::json!({ "model": model, "messages": messages, "stream": true }));
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
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        if req.len() >= h + 4 + len || n == 0 {
                            let body_json = &text[h + 4..];
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(body_json) {
                                seen.lock().unwrap().push(v["model"].as_str().unwrap_or("").to_string());
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
        let body = format!("{}data: {{\"error\":{{\"message\":\"cut\"}}}}\n\n", sse(&["partial"]).replace("data: [DONE]\n\n", ""));
        let (url, models) = fake_seq(vec![("200 OK", body), ("200 OK", sse(&["should not appear"]))]).await;
        let mut cfg = LlmConfig::local(&url, "primary");
        cfg.fallback_model = Some("fallback".into());
        let err = chat_stream(&cfg, &[Message::user("x")], |_| {}).await.unwrap_err();
        assert!(matches!(err, LlmError::Stream(_)), "{err}");
        assert_eq!(*models.lock().unwrap(), ["primary"]);
    }
}
