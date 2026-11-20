//! HTTP mode: the same tools over MCP's Streamable HTTP transport, for a server other devices reach
//! (a homelab on a tailnet), plus an endpoint the app uploads its database to.
//!
//! - `POST /mcp`: JSON-RPC, answered as JSON. Needs the read token.
//! - `GET /status`: when the database was last synced and how many meetings it has. Either token.
//! - `POST /sync`: the body replaces the database (checked first, swapped in atomically). Needs the
//!   sync token, so a device that can read meetings can't overwrite them.
//! - `GET /health`: "ok", no token; for container health checks.
//!
//! Stateless: no sessions, no server-sent events. Every reply is a single JSON body.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response};

use crate::Server;

/// The largest JSON-RPC message accepted.
const MAX_MESSAGE: u64 = 4 << 20;
/// The largest database accepted by /sync.
const MAX_UPLOAD: u64 = 8 << 30;
const WORKERS: usize = 4;

pub struct Config {
    pub addr: String,
    pub read_token: String,
    /// None: /sync is off and the database is managed some other way.
    pub sync_token: Option<String>,
}

pub fn serve(server: Server, cfg: Config) -> Result<(), String> {
    let http = tiny_http::Server::http(&cfg.addr).map_err(|e| format!("couldn't listen on {}: {e}", cfg.addr))?;
    eprintln!(
        "fivesight-mcp: serving {} on http://{}/mcp{}",
        server.db.display(),
        cfg.addr,
        if cfg.sync_token.is_some() { " (sync on)" } else { "" }
    );
    let http = Arc::new(http);
    let app = Arc::new(App { server, cfg, sync_lock: Mutex::new(()) });
    let workers: Vec<_> = (0..WORKERS)
        .map(|_| {
            let (http, app) = (http.clone(), app.clone());
            std::thread::spawn(move || {
                while let Ok(req) = http.recv() {
                    app.respond(req);
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    Ok(())
}

struct App {
    server: Server,
    cfg: Config,
    /// One upload at a time.
    sync_lock: Mutex<()>,
}

struct Reply {
    status: u16,
    body: String,
    json: bool,
}

impl Reply {
    fn json(status: u16, v: Value) -> Self {
        Reply { status, body: v.to_string(), json: true }
    }
    fn text(status: u16, s: &str) -> Self {
        Reply { status, body: s.into(), json: false }
    }
    fn problem(status: u16, s: &str) -> Self {
        Self::json(status, json!({ "error": s }))
    }
}

impl App {
    fn respond(&self, mut req: Request) {
        let path = req.url().split('?').next().unwrap_or("").to_string();
        let reply = self.route(&mut req, &path);
        if reply.status >= 400 {
            eprintln!("{} {path} -> {}", req.method(), reply.status);
        }
        let mut res = Response::from_string(reply.body).with_status_code(reply.status);
        if reply.json {
            res.add_header(header("Content-Type", "application/json"));
        }
        if reply.status == 401 {
            res.add_header(header("WWW-Authenticate", "Bearer"));
        }
        if reply.status == 405 {
            res.add_header(header("Allow", if path == "/mcp" { "POST" } else { "GET" }));
        }
        let _ = req.respond(res);
    }

    fn route(&self, req: &mut Request, path: &str) -> Reply {
        let method = req.method().clone();
        match (path, &method) {
            ("/health", Method::Get) => Reply::text(200, "ok"),
            ("/mcp", Method::Post) => {
                if !self.authorized(req, &[Some(&self.cfg.read_token)]) {
                    return Reply::problem(401, "missing or wrong token");
                }
                self.mcp(req)
            }
            ("/mcp", _) => Reply::problem(405, "this server answers POST only (no event stream)"),
            ("/status", Method::Get) => {
                if !self.authorized(req, &[Some(&self.cfg.read_token), self.cfg.sync_token.as_ref()]) {
                    return Reply::problem(401, "missing or wrong token");
                }
                match status(&self.server.db) {
                    Ok(v) => Reply::json(200, v),
                    Err(e) => Reply::problem(500, &e),
                }
            }
            ("/sync", Method::Post) => {
                let Some(token) = &self.cfg.sync_token else {
                    return Reply::problem(404, "sync is off on this server (no FIVESIGHT_SYNC_TOKEN)");
                };
                if !self.authorized(req, &[Some(token)]) {
                    return Reply::problem(401, "missing or wrong sync token");
                }
                let user = query_param(req.url(), "user");
                let _one = self.sync_lock.lock().unwrap_or_else(|e| e.into_inner());
                match receive_db(&self.server.db, req.as_reader(), user.as_deref()) {
                    Ok(v) => Reply::json(200, v),
                    Err((code, e)) => Reply::problem(code, &e),
                }
            }
            ("/health" | "/status" | "/sync", _) => Reply::problem(405, "wrong method"),
            _ => Reply::problem(404, "not found"),
        }
    }

    fn authorized(&self, req: &Request, tokens: &[Option<&String>]) -> bool {
        let Some(given) = req
            .headers()
            .iter()
            .find(|h| h.field.equiv("Authorization"))
            .and_then(|h| h.value.as_str().strip_prefix("Bearer "))
        else {
            return false;
        };
        tokens.iter().flatten().any(|t| !t.is_empty() && same(given.trim().as_bytes(), t.as_bytes()))
    }

    fn mcp(&self, req: &mut Request) -> Reply {
        let mut body = String::new();
        if req.as_reader().take(MAX_MESSAGE + 1).read_to_string(&mut body).is_err() {
            return Reply::problem(400, "the body isn't UTF-8 text");
        }
        if body.len() as u64 > MAX_MESSAGE {
            return Reply::problem(413, "message too large");
        }
        match self.server.handle_line(&body) {
            Some(reply) => Reply::json(200, reply),
            // A notification or a response: nothing to say back.
            None => Reply { status: 202, body: String::new(), json: false },
        }
    }
}

/// Compare without stopping at the first differing byte, so timing doesn't leak the token.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header")
}

/// When the database file was last replaced, its size and how many meetings it holds.
fn status(db: &Path) -> Result<Value, String> {
    let Ok(meta) = std::fs::metadata(db) else {
        return Ok(json!({ "syncedAt": null, "bytes": 0, "meetings": 0 }));
    };
    let synced_at = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64);
    let meetings = fs_store::looks_like_fivesight_db(db)?;
    Ok(json!({ "syncedAt": synced_at, "bytes": meta.len(), "meetings": meetings }))
}

/// Write the upload next to the database, check it, then rename it over the old one. Readers
/// open the file per call, so the next tool call sees the new data; a failed upload changes nothing.
/// `user` is the name the app calls the user, kept in settings.json as the app does.
fn receive_db(db: &Path, body: &mut dyn Read, user: Option<&str>) -> Result<Value, (u16, String)> {
    let dir = db.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| (500, format!("couldn't create {}: {e}", dir.display())))?;
    let tmp = dir.join(format!(".upload-{}.db", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp).map_err(|e| (500, format!("couldn't write {}: {e}", tmp.display())))?;
        let n = std::io::copy(&mut body.take(MAX_UPLOAD + 1), &mut f)
            .map_err(|e| (400, format!("upload broke off: {e}")))?;
        if n > MAX_UPLOAD {
            return Err((413, "database too large".into()));
        }
        f.sync_all().map_err(|e| (500, e.to_string()))?;
        drop(f);
        fs_store::looks_like_fivesight_db(&tmp).map_err(|e| (422, format!("not taken: {e}")))?;
        std::fs::rename(&tmp, db).map_err(|e| (500, format!("couldn't replace {}: {e}", db.display())))?;
        // A journal left by an old copy must never be applied to the new one.
        for ext in ["-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(PathBuf::from(format!("{}{ext}", db.display())));
        }
        if let Some(name) = user {
            let settings = db.with_file_name("settings.json");
            let tmp_settings = db.with_file_name(".settings.json.tmp");
            let text = json!({ "userName": name.trim() }).to_string();
            let _ = std::fs::write(&tmp_settings, text).and_then(|_| std::fs::rename(&tmp_settings, &settings));
        }
        status(db).map_err(|e| (500, e))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    if let Ok(v) = &result {
        eprintln!("sync: {} meetings, {} bytes", v["meetings"], v["bytes"]);
    }
    result
}

/// `key`'s value from the URL's query string, percent-decoded.
fn query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (decode(k) == key).then(|| decode(v))
    })
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()).ok_or(())
                {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs_store::Store;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fivesight-http-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn query_values_are_decoded() {
        assert_eq!(query_param("/sync?user=Azrial%20Ahmad&x=1", "user").as_deref(), Some("Azrial Ahmad"));
        assert_eq!(query_param("/sync?user=S%C3%A1ri+K", "user").as_deref(), Some("Sári K"));
        assert_eq!(query_param("/sync?user=100%", "user").as_deref(), Some("100%"));
        assert_eq!(query_param("/sync", "user"), None);
    }

    #[test]
    fn tokens_compare_exactly() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"ab"));
        assert!(!same(b"", b"x"));
    }

    #[test]
    fn a_good_upload_replaces_the_database_and_a_bad_one_changes_nothing() {
        let dir = temp("sync");
        let db = dir.join("fivesight.db");
        // The app's side: a database with one meeting, snapshotted.
        let src = Store::open(&dir.join("app.db")).unwrap();
        src.create_meeting("Standup", 1).unwrap();
        let snap = dir.join("snap.db");
        src.snapshot(&snap).unwrap();
        let bytes = std::fs::read(&snap).unwrap();

        let v = receive_db(&db, &mut bytes.as_slice(), Some(" Azrial ")).unwrap();
        assert_eq!(v["meetings"], 1);
        assert!(v["syncedAt"].as_i64().unwrap() > 0);
        let settings = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&settings).unwrap()["userName"], "Azrial");

        let (code, msg) = receive_db(&db, &mut &b"SQLite format 3\0 but then garbage"[..], None).unwrap_err();
        assert_eq!(code, 422, "{msg}");
        assert_eq!(status(&db).unwrap()["meetings"], 1);
        assert!(std::fs::read_dir(&dir).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".upload")));

        // The MCP reads the synced copy.
        let server = Server { db: db.clone() };
        let r = server.call(&json!({ "name": "list_meetings" })).unwrap();
        assert!(r["content"][0]["text"].as_str().unwrap().contains("Standup"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_of_a_server_that_was_never_synced() {
        let dir = temp("never");
        assert_eq!(status(&dir.join("fivesight.db")).unwrap()["syncedAt"], Value::Null);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole thing over a real socket: auth, MCP calls, sync.
    #[test]
    fn end_to_end_over_http() {
        let dir = temp("e2e");
        let db = dir.join("fivesight.db");
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let cfg =
            Config { addr: format!("127.0.0.1:{port}"), read_token: "r3ad".into(), sync_token: Some("syncme".into()) };
        let server = Server { db: db.clone() };
        std::thread::spawn(move || serve(server, cfg));
        std::thread::sleep(std::time::Duration::from_millis(200));

        let call = |method: &str, path: &str, token: Option<&str>, body: &[u8]| -> (u16, String) {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
            let head = format!(
                "{method} {path} HTTP/1.1\r\nHost: x\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            s.write_all(head.as_bytes()).unwrap();
            s.write_all(body).unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            let code = out[9..12].parse().unwrap();
            (code, out.split_once("\r\n\r\n").map(|x| x.1.to_string()).unwrap_or_default())
        };

        assert_eq!(call("GET", "/health", None, b"").0, 200);
        let init = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;
        assert_eq!(call("POST", "/mcp", None, init).0, 401);
        assert_eq!(call("POST", "/mcp", Some("syncme"), init).0, 401, "the sync token can't read");
        let (code, body) = call("POST", "/mcp", Some("r3ad"), init);
        assert_eq!(code, 200);
        assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(
            call("POST", "/mcp", Some("r3ad"), br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).0,
            202
        );
        assert_eq!(call("GET", "/mcp", Some("r3ad"), b"").0, 405);

        let src = Store::open(&dir.join("app.db")).unwrap();
        src.create_meeting("Retro", 1).unwrap();
        src.snapshot(&dir.join("snap.db")).unwrap();
        let snap = std::fs::read(dir.join("snap.db")).unwrap();
        assert_eq!(call("POST", "/sync", Some("r3ad"), &snap).0, 401, "the read token can't write");
        let (code, body) = call("POST", "/sync?user=Azrial", Some("syncme"), &snap);
        assert_eq!(code, 200, "{body}");
        let (code, body) = call("GET", "/status", Some("r3ad"), b"");
        assert_eq!((code, serde_json::from_str::<Value>(&body).unwrap()["meetings"].clone()), (200, json!(1)));

        let list = br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_meetings"}}"#;
        let (_, body) = call("POST", "/mcp", Some("r3ad"), list);
        assert!(body.contains("Retro"), "{body}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
