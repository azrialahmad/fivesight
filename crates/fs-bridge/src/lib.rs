//! fs-bridge: a tiny local HTTP endpoint the Meet browser extension talks to.
//!
//! The extension only reports *who is speaking* and *whether a call is on*. It never sends audio or
//! video. Requests must carry the pairing token and must not come from a web page: a page always
//! sends its own `Origin`, an extension sends `chrome-extension://...`.

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::sync::Arc;
use std::thread::JoinHandle;

use serde::Deserialize;

/// Where the extension looks for the app.
pub const DEFAULT_PORT: u16 = 47285;
const MAX_BODY: usize = 64 * 1024;
/// How long speaker history is kept.
const KEEP_MS: i64 = 3 * 60 * 60 * 1000;
/// Meet's speaking indicator reacts slightly after the voice does; match with this much slack.
const SLACK_MS: i64 = 300;

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("couldn't listen on {0}: {1}")]
    Bind(String, String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CallState {
    Joined,
    Left,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Incoming {
    /// Heartbeat from the extension (every 30 s or so, Meet tab or not), so the app can show
    /// "connected". `version` is the extension's, so the app can tell when it's out of date.
    Hello {
        #[serde(default)]
        version: Option<String>,
        /// Sent by an open Meet tab (not just the background worker).
        #[serde(default)]
        tab: bool,
    },
    /// A participant started or stopped speaking. `at` is the browser's `Date.now()`.
    Speaking { name: String, speaking: bool, at: i64 },
    /// The user joined or left a call in this tab.
    Call {
        state: CallState,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        code: Option<String>,
    },
}

#[derive(Deserialize)]
struct Batch {
    events: Vec<Incoming>,
}

// ---------- who was speaking when ----------

#[derive(Debug, Clone)]
struct Interval {
    name: String,
    start: i64,
    end: i64,
}

/// Remembers when each participant was speaking, to name transcript lines afterwards.
#[derive(Debug, Default)]
pub struct SpeakerLog {
    open: HashMap<String, i64>,
    closed: VecDeque<Interval>,
}

impl SpeakerLog {
    pub fn apply(&mut self, name: &str, speaking: bool, at: i64) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        if speaking {
            self.open.entry(name.to_string()).or_insert(at);
        } else if let Some(start) = self.open.remove(name) {
            self.closed.push_back(Interval { name: name.to_string(), start, end: at.max(start) });
        }
        while self.closed.front().is_some_and(|i| i.end < at - KEEP_MS) {
            self.closed.pop_front();
        }
    }

    /// Everyone's indicators go dark (the call ended or the tab closed).
    pub fn stop_all(&mut self, at: i64) {
        let names: Vec<String> = self.open.keys().cloned().collect();
        for n in names {
            self.apply(&n, false, at);
        }
    }

    /// Who spoke the most between `start` and `end` (epoch ms), if anyone clearly did.
    pub fn speaker_for(&self, start: i64, end: i64) -> Option<String> {
        let end = end.max(start);
        let (from, to) = (start - SLACK_MS, end + SLACK_MS);
        let mut total: HashMap<String, i64> = HashMap::new();
        for i in &self.closed {
            let overlap = i.end.min(to) - i.start.max(from);
            if overlap > 0 {
                *total.entry(i.name.clone()).or_default() += overlap;
            }
        }
        // Someone still talking counts up to the end of the window.
        for (name, s) in &self.open {
            let overlap = to - (*s).max(from);
            if overlap > 0 {
                *total.entry(name.clone()).or_default() += overlap;
            }
        }
        let need = ((end - start) / 3).clamp(150, 400);
        total
            .into_iter()
            .filter(|(_, ms)| *ms >= need)
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
            .map(|(n, _)| n)
    }
}

// ---------- request handling ----------

/// What the router needs from an HTTP request.
pub struct RequestParts<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub origin: Option<&'a str>,
    pub authorization: Option<&'a str>,
    pub body: &'a [u8],
}

#[derive(Debug, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub body: &'static str,
    pub events: Vec<Incoming>,
}

fn reply(status: u16, body: &'static str) -> Reply {
    Reply { status, body, events: Vec::new() }
}

/// Compares without stopping at the first difference, so a wrong token doesn't leak how much matched.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn route(req: &RequestParts, token: &str) -> Reply {
    if let Some(origin) = req.origin {
        if !(origin.starts_with("chrome-extension://") || origin.starts_with("moz-extension://")) {
            return reply(403, "web pages can't talk to fivesight");
        }
    }
    let presented = req.authorization.and_then(|h| h.strip_prefix("Bearer ")).unwrap_or("");
    if token.is_empty() || !same(presented, token) {
        return reply(401, "wrong or missing pairing token");
    }
    if req.method != "POST" {
        return reply(405, "use POST");
    }
    match req.path {
        "/ping" => reply(200, "ok"),
        "/events" => {
            if req.body.len() > MAX_BODY {
                return reply(413, "too large");
            }
            match serde_json::from_slice::<Batch>(req.body) {
                Ok(b) => Reply { status: 200, body: "ok", events: b.events },
                Err(_) => reply(400, "bad events"),
            }
        }
        _ => reply(404, "not found"),
    }
}

// ---------- the server ----------

pub struct Bridge {
    server: Arc<tiny_http::Server>,
    thread: Option<JoinHandle<()>>,
}

impl Bridge {
    /// Listen on `127.0.0.1:port` (0 picks a free port). `on_event` runs on the server thread.
    pub fn start(port: u16, token: String, on_event: impl Fn(Incoming) + Send + 'static) -> Result<Self, BridgeError> {
        let addr = format!("127.0.0.1:{port}");
        let server = tiny_http::Server::http(&addr).map_err(|e| BridgeError::Bind(addr, e.to_string()))?;
        let server = Arc::new(server);
        let worker = server.clone();
        let thread = std::thread::spawn(move || {
            for mut request in worker.incoming_requests() {
                let mut body = Vec::new();
                let _ = request.as_reader().take(MAX_BODY as u64 + 1).read_to_end(&mut body);
                let header = |name: &str| {
                    request
                        .headers()
                        .iter()
                        .find(|h| h.field.to_string().eq_ignore_ascii_case(name))
                        .map(|h| h.value.as_str().to_string())
                };
                let (origin, auth) = (header("Origin"), header("Authorization"));
                let method = request.method().as_str().to_string();
                let path = request.url().split('?').next().unwrap_or("").to_string();
                let out = route(
                    &RequestParts {
                        method: &method,
                        path: &path,
                        origin: origin.as_deref(),
                        authorization: auth.as_deref(),
                        body: &body,
                    },
                    &token,
                );
                for ev in out.events {
                    on_event(ev);
                }
                let _ = request.respond(tiny_http::Response::from_string(out.body).with_status_code(out.status));
            }
        });
        Ok(Self { server, thread: Some(thread) })
    }

    pub fn port(&self) -> Option<u16> {
        self.server.server_addr().to_ip().map(|a| a.port())
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpStream;
    use std::sync::mpsc;
    use std::time::Duration;

    const TOKEN: &str = "s3cret-token";

    fn post<'a>(path: &'a str, origin: Option<&'a str>, auth: Option<&'a str>, body: &'a [u8]) -> RequestParts<'a> {
        RequestParts { method: "POST", path, origin, authorization: auth, body }
    }

    #[test]
    fn rejects_web_pages_even_with_the_token() {
        let r = route(&post("/ping", Some("https://evil.example"), Some("Bearer s3cret-token"), b""), TOKEN);
        assert_eq!(r.status, 403);
        let r = route(&post("/ping", Some("https://meet.google.com"), Some("Bearer s3cret-token"), b""), TOKEN);
        assert_eq!(r.status, 403, "not even Meet's own page may call it directly");
    }

    #[test]
    fn requires_the_pairing_token() {
        for auth in [None, Some("Bearer nope"), Some("s3cret-token"), Some("Bearer ")] {
            assert_eq!(route(&post("/ping", Some("chrome-extension://abc"), auth, b""), TOKEN).status, 401);
        }
        assert_eq!(
            route(&post("/ping", Some("chrome-extension://abc"), Some("Bearer s3cret-token"), b""), TOKEN).status,
            200
        );
        // An unset token must never match an empty header.
        assert_eq!(route(&post("/ping", None, None, b""), "").status, 401);
    }

    #[test]
    fn parses_event_batches() {
        let body = br#"{"events":[
            {"type":"hello"},
            {"type":"speaking","name":"Budi","speaking":true,"at":1000},
            {"type":"call","state":"joined","title":"Payments sync","code":"abc-defg-hij"},
            {"type":"call","state":"left"}
        ]}"#;
        let r = route(&post("/events", Some("chrome-extension://abc"), Some("Bearer s3cret-token"), body), TOKEN);
        assert_eq!(r.status, 200);
        assert_eq!(r.events.len(), 4);
        assert_eq!(r.events[0], Incoming::Hello { version: None, tab: false }); // extensions before 0.2 send neither
        let v = route(
            &post(
                "/events",
                Some("chrome-extension://abc"),
                Some("Bearer s3cret-token"),
                br#"{"events":[{"type":"hello","version":"0.2.0","tab":true}]}"#,
            ),
            TOKEN,
        );
        assert_eq!(v.events, vec![Incoming::Hello { version: Some("0.2.0".into()), tab: true }]);
        assert_eq!(r.events[1], Incoming::Speaking { name: "Budi".into(), speaking: true, at: 1000 });
        assert_eq!(
            r.events[2],
            Incoming::Call {
                state: CallState::Joined,
                title: Some("Payments sync".into()),
                code: Some("abc-defg-hij".into())
            }
        );
    }

    #[test]
    fn rejects_garbage_oversize_and_wrong_routes() {
        let auth = Some("Bearer s3cret-token");
        let o = Some("chrome-extension://abc");
        assert_eq!(route(&post("/events", o, auth, b"not json"), TOKEN).status, 400);
        assert_eq!(route(&post("/events", o, auth, br#"{"events":[{"type":"explode"}]}"#), TOKEN).status, 400);
        assert_eq!(route(&post("/events", o, auth, &vec![b' '; MAX_BODY + 1]), TOKEN).status, 413);
        assert_eq!(route(&post("/other", o, auth, b""), TOKEN).status, 404);
        let get = RequestParts { method: "GET", path: "/ping", origin: o, authorization: auth, body: b"" };
        assert_eq!(route(&get, TOKEN).status, 405);
    }

    #[test]
    fn names_the_person_who_spoke_most_in_the_window() {
        let mut log = SpeakerLog::default();
        log.apply("Budi", true, 1_000);
        log.apply("Budi", false, 4_000);
        log.apply("Sari", true, 4_200);
        log.apply("Sari", false, 9_000);
        assert_eq!(log.speaker_for(1_200, 3_800).as_deref(), Some("Budi"));
        assert_eq!(log.speaker_for(4_500, 8_500).as_deref(), Some("Sari"));
        // Straddles both: Sari talked far longer inside the window.
        assert_eq!(log.speaker_for(3_000, 8_000).as_deref(), Some("Sari"));
        // Nobody was speaking.
        assert_eq!(log.speaker_for(20_000, 22_000), None);
    }

    #[test]
    fn a_speaker_still_talking_counts_up_to_now() {
        let mut log = SpeakerLog::default();
        log.apply("Budi", true, 1_000);
        assert_eq!(log.speaker_for(2_000, 4_000).as_deref(), Some("Budi"));
        log.stop_all(5_000);
        assert_eq!(log.speaker_for(2_000, 4_000).as_deref(), Some("Budi"));
        assert_eq!(log.speaker_for(9_000, 10_000), None);
    }

    #[test]
    fn blips_and_blanks_are_ignored() {
        let mut log = SpeakerLog::default();
        log.apply("", true, 0);
        log.apply("  ", false, 10);
        // A 50 ms flicker is not a speaker for a 3 s sentence.
        log.apply("Andi", true, 10_000);
        log.apply("Andi", false, 10_050);
        assert_eq!(log.speaker_for(5_000, 8_000), None);
        assert_eq!(log.speaker_for(9_000, 12_000), None);
    }

    #[test]
    fn old_history_is_pruned() {
        let mut log = SpeakerLog::default();
        log.apply("Budi", true, 0);
        log.apply("Budi", false, 1_000);
        log.apply("Sari", true, KEEP_MS + 50_000);
        log.apply("Sari", false, KEEP_MS + 51_000);
        assert_eq!(log.closed.len(), 1);
    }

    fn http(port: u16, request: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(request.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    #[test]
    fn serves_over_a_real_socket_and_shuts_down() {
        let (tx, rx) = mpsc::channel();
        let bridge = Bridge::start(0, TOKEN.into(), move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        let port = bridge.port().unwrap();
        let body = r#"{"events":[{"type":"speaking","name":"Budi","speaking":true,"at":5}]}"#;
        let req = |auth: &str, origin: &str| {
            format!(
                "POST /events HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: {origin}\r\nAuthorization: {auth}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        };
        assert!(http(port, &req("Bearer wrong", "chrome-extension://x")).starts_with("HTTP/1.1 401"));
        assert!(http(port, &req("Bearer s3cret-token", "https://evil.example")).starts_with("HTTP/1.1 403"));
        assert!(rx.try_recv().is_err(), "rejected requests must not reach the handler");
        assert!(http(port, &req("Bearer s3cret-token", "chrome-extension://x")).starts_with("HTTP/1.1 200"));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Incoming::Speaking { name: "Budi".into(), speaking: true, at: 5 }
        );
        drop(bridge);
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err(), "port should be released");
    }
}
