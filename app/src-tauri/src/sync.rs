//! Sync to your own server: upload a copy of the database to a `fivesight-mcp --http` server (a
//! homelab on a tailnet, say), so agents on other devices can read your meetings while this PC is off.
//!
//! One way and whole: the server's copy is replaced by this PC's each time. Nothing comes back.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Credential Manager slot for the server's sync token.
pub const TOKEN_SLOT: &str = "sync-token";

/// What the server has now.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    /// When its copy was last replaced (epoch ms); None if it was never synced.
    pub synced_at: Option<i64>,
    pub bytes: u64,
    pub meetings: usize,
}

/// The server's base address, however it was pasted: with or without a trailing slash or /mcp.
pub fn base_url(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    url.strip_suffix("/mcp").unwrap_or(url).trim_end_matches('/').to_string()
}

fn client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())
}

/// Send the database file's bytes. `user` is the name transcripts call you, so the server's copy
/// labels your lines the same way.
pub async fn upload(url: &str, token: &str, user: Option<&str>, db: Vec<u8>) -> Result<SyncStatus, String> {
    let base = base_url(url);
    let mut target = reqwest::Url::parse(&format!("{base}/sync")).map_err(|e| bad_address(&base, e))?;
    if let Some(name) = user {
        target.query_pairs_mut().append_pair("user", name);
    }
    let res = client(Duration::from_secs(600))?
        .post(target)
        .bearer_auth(token.trim())
        .header("Content-Type", "application/vnd.sqlite3")
        .body(db)
        .send()
        .await
        .map_err(|e| unreachable(&base, e))?;
    read(res, "sync").await
}

/// Ask the server what it has, to show "Last synced …" and to check the address and token work.
pub async fn status(url: &str, token: &str) -> Result<SyncStatus, String> {
    let base = base_url(url);
    let target = reqwest::Url::parse(&format!("{base}/status")).map_err(|e| bad_address(&base, e))?;
    let res = client(Duration::from_secs(15))?
        .get(target)
        .bearer_auth(token.trim())
        .send()
        .await
        .map_err(|e| unreachable(&base, e))?;
    read(res, "status").await
}

async fn read(res: reqwest::Response, what: &str) -> Result<SyncStatus, String> {
    let code = res.status();
    let text = res.text().await.unwrap_or_default();
    if code.is_success() {
        return serde_json::from_str(&text)
            .map_err(|_| "The server answered, but not like a fivesight server. Is the address right?".into());
    }
    let detail = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("error")?.as_str().map(String::from))
        .unwrap_or_else(|| text.chars().take(200).collect());
    Err(match code.as_u16() {
        401 => "The server didn't accept the sync token. Paste it again from the server's settings.".into(),
        404 if what == "sync" && detail.contains("sync is off") => {
            "Sync is off on that server: it needs FIVESIGHT_SYNC_TOKEN set.".into()
        }
        404 => "Nothing at that address answers like a fivesight server. Check the address and port.".into(),
        _ => format!("The server said {code}: {detail}"),
    })
}

fn bad_address(base: &str, e: impl std::fmt::Display) -> String {
    format!("\"{base}\" isn't a web address ({e}). It looks like http://your-server:8787.")
}

fn unreachable(base: &str, e: reqwest::Error) -> String {
    if e.is_timeout() {
        format!("{base} took too long to answer.")
    } else {
        format!(
            "Couldn't reach {base}. Is the server running, and is this PC on the same network (Tailscale connected)?"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_tidied() {
        assert_eq!(base_url(" http://asus-homelab:8787/ "), "http://asus-homelab:8787");
        assert_eq!(base_url("https://h.ts.net:8787/mcp"), "https://h.ts.net:8787");
        assert_eq!(base_url("https://h.ts.net:8787/mcp/"), "https://h.ts.net:8787");
    }

    #[test]
    fn status_reads_the_servers_reply() {
        let s: SyncStatus = serde_json::from_str(r#"{"syncedAt":1,"bytes":2,"meetings":3}"#).unwrap();
        assert_eq!(s, SyncStatus { synced_at: Some(1), bytes: 2, meetings: 3 });
        let never: SyncStatus = serde_json::from_str(r#"{"syncedAt":null,"bytes":0,"meetings":0}"#).unwrap();
        assert_eq!(never.synced_at, None);
    }
}
