//! fs-bridge: a tiny local HTTP endpoint the Meet browser extension talks to.
//!
//! The extension reports *who is speaking*, *whether a call is on* and *what is typed in the call's
//! chat*. It never sends audio or video. Requests must carry the pairing token and must not come
//! from a web page: a page always sends its own `Origin`, an extension sends `chrome-extension://...`.

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
        /// Your own display name in Meet, once a call tab has seen it.
        #[serde(default)]
        me: Option<String>,
    },
    /// A participant started or stopped speaking. `at` is the browser's `Date.now()`.
    Speaking { name: String, speaking: bool, at: i64 },
    /// A message in the call's chat. `id` is Meet's, so a message reported twice can be dropped.
    /// `at` is the browser's `Date.now()` (or the minute Meet printed, for one found late).
    Chat {
        id: String,
        /// Display name, when the extension could read it.
        #[serde(default)]
        sender: Option<String>,
        text: String,
        at: i64,
        /// You sent it.
        #[serde(default)]
        me: bool,
    },
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

/// Someone starting to talk this soon after another person stopped is taking over from them.
const TAKEOVER_MS: i64 = 1_500;
/// How late a start is reported: Meet's indicator, then the extension wants a few changes over
/// two ticks before it believes it (see `createSpeakingTracker`).
const START_LAG_MS: i64 = 500;
/// How late a stop is reported: the extension waits five quiet ticks (a second) first.
const END_LAG_MS: i64 = 1_000;
/// When two people both covered a line, someone who covered at least this share of what the
/// other did counts as close; the one who started later then gets it, since they cut in.
const CLOSE_SHARE: f32 = 0.7;

impl SpeakerLog {
    /// Record a change reported at `at`. Returns when the line on the call should be cut (epoch
    /// ms, corrected for the report's delay), if this change is one: someone starts while
    /// someone else is talking or just stopped (a takeover), or someone who cut in stops while
    /// the person they interrupted is still talking (who then has the floor back).
    pub fn apply(&mut self, name: &str, speaking: bool, at: i64) -> Option<i64> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let mut cut = None;
        if speaking {
            if !self.open.contains_key(name) {
                let start = at - START_LAG_MS;
                let takeover = self.open.keys().any(|n| n != name)
                    || self.closed.back().is_some_and(|i| i.name != name && start - i.end <= TAKEOVER_MS);
                cut = takeover.then_some(start);
                self.open.insert(name.to_string(), start);
            }
        } else if let Some(start) = self.open.remove(name) {
            let end = (at - END_LAG_MS).max(start);
            // Still talking, and was already when this person came in: they were interrupted.
            if self.open.values().any(|&s| s < start) {
                cut = Some(end);
            }
            self.closed.push_back(Interval { name: name.to_string(), start, end });
        }
        while self.closed.front().is_some_and(|i| i.end < at - KEEP_MS) {
            self.closed.pop_front();
        }
        cut
    }

    /// Everyone's indicators go dark (the call ended or the tab closed).
    pub fn stop_all(&mut self, at: i64) {
        let names: Vec<String> = self.open.keys().cloned().collect();
        for n in names {
            self.apply(&n, false, at);
        }
    }

    /// Who spoke between `start` and `end` (epoch ms), if anyone clearly did: whoever spoke the
    /// most, except that when two covered it about as much (one was cutting in on the other),
    /// the one who started later, since the interrupted person's indicator stays lit over it.
    pub fn speaker_for(&self, start: i64, end: i64) -> Option<String> {
        let end = end.max(start);
        let (from, to) = (start - SLACK_MS, end + SLACK_MS);
        // name -> (time covered, latest start of a turn that covered it)
        let mut total: HashMap<String, (i64, i64)> = HashMap::new();
        let mut add = |name: &str, s: i64, e: i64| {
            let overlap = e.min(to) - s.max(from);
            if overlap > 0 {
                let t = total.entry(name.to_string()).or_insert((0, s));
                t.0 += overlap;
                t.1 = t.1.max(s);
            }
        };
        for i in &self.closed {
            add(&i.name, i.start, i.end);
        }
        // Someone still talking counts up to the end of the window.
        for (name, &s) in &self.open {
            add(name, s, to);
        }
        let need = ((end - start) / 3).clamp(150, 400);
        let most = total.values().map(|t| t.0).max().filter(|&ms| ms >= need)?;
        let close = (most as f32 * CLOSE_SHARE) as i64;
        total
            .into_iter()
            .filter(|(_, (ms, _))| *ms >= close.max(need))
            .max_by(|a, b| a.1 .1.cmp(&b.1 .1).then(a.1 .0.cmp(&b.1 .0)).then_with(|| b.0.cmp(&a.0)))
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
        assert_eq!(r.events[0], Incoming::Hello { version: None, tab: false, me: None }); // extensions before 0.2 send neither
        let v = route(
            &post(
                "/events",
                Some("chrome-extension://abc"),
                Some("Bearer s3cret-token"),
                br#"{"events":[{"type":"hello","version":"0.2.0","tab":true,"me":"Azrial"}]}"#,
            ),
            TOKEN,
        );
        assert_eq!(
            v.events,
            vec![Incoming::Hello { version: Some("0.2.0".into()), tab: true, me: Some("Azrial".into()) }]
        );
        assert_eq!(r.events[1], Incoming::Speaking { name: "Budi".into(), speaking: true, at: 1000 });
        assert_eq!(
            r.events[2],
            Incoming::Call {
                state: CallState::Joined,
                title: Some("Payments sync".into()),
                code: Some("abc-defg-hij".into())
            }
        );
        let c = route(
            &post(
                "/events",
                Some("chrome-extension://abc"),
                Some("Bearer s3cret-token"),
                br#"{"events":[
                    {"type":"chat","id":"m1","sender":"Budi","text":"PAY-142 is the one","at":2000},
                    {"type":"chat","id":"m2","text":"hi","at":3000,"me":true}
                ]}"#,
            ),
            TOKEN,
        );
        assert_eq!(
            c.events,
            vec![
                Incoming::Chat {
                    id: "m1".into(),
                    sender: Some("Budi".into()),
                    text: "PAY-142 is the one".into(),
                    at: 2000,
                    me: false
                },
                Incoming::Chat { id: "m2".into(), sender: None, text: "hi".into(), at: 3000, me: true },
            ]
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
    fn says_when_someone_else_takes_over() {
        let mut log = SpeakerLog::default();
        assert_eq!(log.apply("Budi", true, 1_000), None, "first speaker");
        assert_eq!(log.apply("Budi", true, 1_200), None, "still Budi");
        // Cut where Sari really started: half a second before it was reported.
        assert_eq!(log.apply("Sari", true, 3_000), Some(2_500), "over Budi");
        assert_eq!(log.apply("Budi", false, 3_100), None, "Budi was there first: no hand-back");
        log.apply("Sari", false, 5_000);
        assert_eq!(log.apply("Sari", true, 5_500), None, "Sari again after a breath");
        log.apply("Sari", false, 6_000);
        assert_eq!(log.apply("Budi", true, 7_000), Some(6_500), "right after Sari stopped");
        log.apply("Budi", false, 8_000);
        assert_eq!(log.apply("Sari", true, 20_000), None, "after a long silence, nothing to cut");
        assert_eq!(log.apply("  ", true, 20_100), None);
    }

    #[test]
    fn the_floor_goes_back_after_an_interruption() {
        let mut log = SpeakerLog::default();
        log.apply("Budi", true, 1_000);
        assert_eq!(log.apply("Sari", true, 5_000), Some(4_500), "Sari cuts in");
        // Sari stops while Budi carries on: cut there too, a second before it was reported.
        assert_eq!(log.apply("Sari", false, 8_000), Some(7_000));
        // The lines on either side go to the right people, though Budi's light never went out.
        assert_eq!(log.speaker_for(1_000, 4_400).as_deref(), Some("Budi"));
        assert_eq!(log.speaker_for(4_500, 7_000).as_deref(), Some("Sari"));
        assert_eq!(log.speaker_for(7_000, 10_000).as_deref(), Some("Budi"));
    }

    #[test]
    fn a_quick_reply_is_not_named_after_the_last_speaker() {
        let mut log = SpeakerLog::default();
        // Budi stops at 10.0 s, Sari answers "iya, bisa" from 10.4 to 11.4 s. The extension
        // reports each a little late: Budi's stop at 11.0, Sari's start at 10.9 and stop at 12.4.
        log.apply("Budi", true, 2_500);
        log.apply("Budi", false, 11_000);
        log.apply("Sari", true, 10_900);
        log.apply("Sari", false, 12_400);
        assert_eq!(log.speaker_for(10_400, 11_400).as_deref(), Some("Sari"));
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
        // tiny_http closes its listening socket on its own accept thread, which it doesn't wait
        // for, so the port can stay open a moment after the drop. It must close soon after.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while TcpStream::connect(("127.0.0.1", port)).is_ok() {
            assert!(std::time::Instant::now() < deadline, "port should be released");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
