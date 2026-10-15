//! Read-only Confluence through `confluence-cli` (the npm `confluence` command,
//! <https://github.com/pchuri/confluence-cli>), which can search, unlike `acli`.
//!
//! The same two checks as for `acli` apply: typed commands build fixed argument lists, and
//! [`validate_argv`] checks every argument against an exact allowlist before anything runs.
//! The CLI's write commands (`create`, `update`, `delete`, `comment`, `attachment-upload`,
//! `property-set`, `move`, `copy-tree`, ...) and its raw `api` and file-writing `export`
//! commands are never allowed.
//!
//! The CLI is a Node script. It is started as `node <script>` rather than through its `.cmd`
//! shim, so no `cmd.exe` ever parses the arguments.

use std::path::PathBuf;

use crate::AtlassianError;

/// Comment threads (inline, footer and resolved) are where decisions often end up.
const COMMENT_LOCATIONS: &str = "inline,footer,resolved";

struct Allowed {
    verb: &'static str,
    positional: usize,
    /// Flags that take a value, with the values allowed (empty = any value not starting with '-').
    value_flags: &'static [(&'static str, &'static [&'static str])],
    bool_flags: &'static [&'static str],
}

/// The complete list of what may run. Nothing that changes data is here.
const ALLOWED: &[Allowed] = &[
    Allowed { verb: "search", positional: 1, value_flags: &[("--limit", &[])], bool_flags: &[] },
    Allowed { verb: "find", positional: 1, value_flags: &[("--space", &[])], bool_flags: &[] },
    // Storage format is converted by fivesight (see `storage`); Markdown is the fallback.
    Allowed { verb: "read", positional: 1, value_flags: &[("--format", &["storage", "markdown"])], bool_flags: &[] },
    // Title and space of a page.
    Allowed { verb: "info", positional: 1, value_flags: &[], bool_flags: &[] },
    Allowed {
        verb: "comments",
        positional: 1,
        value_flags: &[("--format", &["markdown"]), ("--depth", &["all"]), ("--location", &[COMMENT_LOCATIONS])],
        bool_flags: &["--all"],
    },
    // Only used to check that the CLI is set up.
    Allowed { verb: "spaces", positional: 0, value_flags: &[("--limit", &["1"])], bool_flags: &[] },
];

/// Check a full argument list (after the script path) against the allowlist.
pub fn validate_argv(argv: &[String]) -> Result<(), AtlassianError> {
    let reject = || AtlassianError::NotAllowed(format!("confluence {}", argv.join(" ")));
    if argv.is_empty() || argv.iter().any(|a| a.contains('\0')) {
        return Err(reject());
    }
    let spec = ALLOWED.iter().find(|a| a.verb == argv[0]).ok_or_else(reject)?;
    let mut positional = 0;
    let mut rest = argv[1..].iter();
    while let Some(tok) = rest.next() {
        if let Some((_, allowed)) = spec.value_flags.iter().find(|(f, _)| f == tok) {
            match rest.next() {
                Some(v)
                    if !v.is_empty()
                        && !v.starts_with('-')
                        && (allowed.is_empty() || allowed.contains(&v.as_str())) => {}
                _ => return Err(reject()),
            }
        } else if spec.bool_flags.contains(&tok.as_str()) {
        } else if !tok.starts_with('-') && positional < spec.positional {
            positional += 1;
        } else {
            return Err(reject());
        }
    }
    if positional != spec.positional {
        return Err(reject());
    }
    Ok(())
}

fn check_text(name: &str, v: &str) -> Result<(), AtlassianError> {
    let v = v.trim();
    if v.is_empty() || v.len() > 500 || v.contains('\0') || v.starts_with('-') {
        return Err(AtlassianError::InvalidArg(format!("{name}: {v:?}")));
    }
    Ok(())
}

pub fn search_argv(query: &str, limit: u32) -> Result<Vec<String>, AtlassianError> {
    check_text("Confluence search", query)?;
    Ok(vec!["search".into(), query.trim().into(), "--limit".into(), limit.clamp(1, 20).to_string()])
}

pub fn find_argv(title: &str, space: Option<&str>) -> Result<Vec<String>, AtlassianError> {
    check_text("page title", title)?;
    let mut v = vec!["find".to_string(), title.trim().to_string()];
    if let Some(space) = space.map(str::trim).filter(|s| !s.is_empty()) {
        if space.len() > 50 || !space.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '~') {
            return Err(AtlassianError::InvalidArg(format!("space key: {space:?}")));
        }
        v.extend(["--space".to_string(), space.to_string()]);
    }
    Ok(v)
}

fn check_page_id(id: &str) -> Result<(), AtlassianError> {
    if id.is_empty() || id.len() > 20 || !id.chars().all(|c| c.is_ascii_digit()) {
        return Err(AtlassianError::InvalidArg(format!("page id: {id:?}")));
    }
    Ok(())
}

/// The page body in storage format (XHTML), which keeps lists inside table cells.
pub fn read_argv(page_id: &str) -> Result<Vec<String>, AtlassianError> {
    check_page_id(page_id)?;
    Ok(vec!["read".into(), page_id.into(), "--format".into(), "storage".into()])
}

/// The CLI's own Markdown, for when the storage body can't be read.
pub fn read_markdown_argv(page_id: &str) -> Result<Vec<String>, AtlassianError> {
    check_page_id(page_id)?;
    Ok(vec!["read".into(), page_id.into(), "--format".into(), "markdown".into()])
}

pub fn info_argv(page_id: &str) -> Result<Vec<String>, AtlassianError> {
    check_page_id(page_id)?;
    Ok(vec!["info".into(), page_id.into()])
}

/// The title from `info` output (`Title: Laporan Keuangan`).
pub fn title_from_info(out: &str) -> Option<String> {
    out.lines().find_map(|l| l.trim().strip_prefix("Title:").map(|t| t.trim().to_string())).filter(|t| !t.is_empty())
}

/// Comment listings carry ids, marker refs and version counters nobody reads; keep who, when,
/// whether it's resolved, what it's about, and what was said.
pub fn tidy_comments(out: &str) -> String {
    let mut lines = Vec::new();
    for line in out.lines() {
        let t = line.trim();
        if t.starts_with("Marker ref:") || t == "Body:" {
            continue;
        }
        if let Some(meta) = t.strip_prefix("Created:") {
            let date: String = meta.trim().chars().take(10).collect();
            let resolved = meta.contains("Resolution: resolved");
            let indent = &line[..line.len() - line.trim_start().len()];
            lines.push(format!("{indent}{date}{}", if resolved { " (resolved)" } else { "" }));
            continue;
        }
        // "1. Name (ID: 123) [inline]" -> "1. Name [inline]"
        let line = match (line.find(" (ID: "), line.find(')')) {
            (Some(a), Some(b)) if b > a => format!("{}{}", &line[..a], &line[b + 1..]),
            _ => line.to_string(),
        };
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n").trim().to_string()
}

pub fn comments_argv(page_id: &str) -> Result<Vec<String>, AtlassianError> {
    check_page_id(page_id)?;
    Ok(vec![
        "comments".into(),
        page_id.into(),
        "--format".into(),
        "markdown".into(),
        "--all".into(),
        "--depth".into(),
        "all".into(),
        "--location".into(),
        COMMENT_LOCATIONS.into(),
    ])
}

pub fn status_argv() -> Vec<String> {
    vec!["spaces".into(), "--limit".into(), "1".into()]
}

// ---------- finding the CLI ----------

fn file(p: PathBuf) -> Option<PathBuf> {
    p.is_file().then_some(p)
}

/// The CLI's entry script: `FIVESIGHT_CONFLUENCE_CLI`, then a Volta install, then a global npm install.
fn find_script() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("FIVESIGHT_CONFLUENCE_CLI").map(PathBuf::from).and_then(file) {
        return Some(p);
    }
    let rel = ["node_modules", "confluence-cli", "bin", "index.js"];
    let join = |base: PathBuf| rel.iter().fold(base, |p, r| p.join(r));
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let roaming = std::env::var_os("APPDATA").map(PathBuf::from);
    let candidates = [
        local.map(|d| join(d.join("Volta").join("tools").join("image").join("packages").join("confluence-cli"))),
        roaming.map(|d| join(d.join("npm"))),
    ];
    candidates.into_iter().flatten().find_map(file)
}

/// `node`: on `PATH` (including Volta's shim), else the newest Node that Volta has downloaded.
fn find_node() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "node.exe" } else { "node" };
    if let Some(paths) = std::env::var_os("PATH") {
        if let Some(p) = std::env::split_paths(&paths).map(|d| d.join(exe)).find(|p| p.is_file()) {
            return Some(p);
        }
    }
    let dir = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Volta").join("tools").join("image").join("node");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path().join(exe)).collect();
    versions.sort();
    versions.into_iter().rev().find_map(file)
}

/// `node` and the CLI's script, if both are installed.
pub fn find() -> Option<(PathBuf, PathBuf)> {
    Some((find_node()?, find_script()?))
}

/// The CLI reports some failures on stdout with exit code 0; spot them.
pub fn looks_like_error(out: &str) -> bool {
    let t = out.trim_start();
    t.starts_with('❌') || t.starts_with("Error:")
}

/// Whether a failure means "not set up yet" rather than a real error.
pub fn is_auth_problem(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("requires a token") || m.contains("confluence init") || m.contains("401") || m.contains("unauthorized")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn every_command_fivesight_builds_passes_the_allowlist() {
        for argv in [
            search_argv("checkout timeout", 5).unwrap(),
            find_argv("Payments runbook", Some("ENG")).unwrap(),
            find_argv("Payments runbook", None).unwrap(),
            read_argv("123456").unwrap(),
            read_markdown_argv("123456").unwrap(),
            info_argv("123456").unwrap(),
            comments_argv("123456").unwrap(),
            status_argv(),
        ] {
            validate_argv(&argv).unwrap();
        }
    }

    #[test]
    fn write_and_raw_commands_are_rejected() {
        // Every command confluence-cli 2.29 has that can change something, plus `api`
        // (arbitrary requests) and `export` (writes files).
        for bad in [
            v(&["create", "Title", "ENG"]),
            v(&["create-child", "Title", "1"]),
            v(&["update", "1"]),
            v(&["move", "1", "2"]),
            v(&["delete", "1"]),
            v(&["version-delete", "1", "2"]),
            v(&["versions-purge", "1"]),
            v(&["attachment-upload", "1"]),
            v(&["attachment-delete", "1", "2"]),
            v(&["property-set", "1", "k"]),
            v(&["property-delete", "1", "k"]),
            v(&["comment", "1"]),
            v(&["comment-resolve", "1"]),
            v(&["comment-delete", "1"]),
            v(&["copy-tree", "1", "2"]),
            v(&["api", "/wiki/rest/api/content", "--method", "POST"]),
            v(&["export", "1"]),
            v(&["edit", "1"]),
            v(&["init"]),
            v(&["profile", "remove", "default"]),
            v(&["install-skill"]),
            v(&["attachments", "1", "--download"]),
            v(&[]),
        ] {
            assert!(matches!(validate_argv(&bad), Err(AtlassianError::NotAllowed(_))), "{bad:?}");
        }
    }

    #[test]
    fn unexpected_flags_and_values_are_rejected() {
        for bad in [
            v(&["read", "1", "--format", "html"]),
            v(&["read", "1", "--format", "storage", "--format", "export"]),
            v(&["read", "1", "--front-matter", "x"]),
            v(&["info", "1", "2"]),
            v(&["info", "1", "--json"]),
            v(&["search", "q", "--cql"]),
            v(&["search", "q", "r"]),
            v(&["search", "--limit", "5"]),
            v(&["search", "q", "--limit"]),
            v(&["comments", "1", "--location", "inline"]),
            v(&["comments", "1", "--limit", "5"]),
            v(&["spaces", "--all"]),
            v(&["spaces", "--limit", "500"]),
            v(&["find", "x", "--space", "--all"]),
        ] {
            assert!(validate_argv(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn inputs_from_the_ai_cannot_smuggle_flags() {
        assert!(search_argv("--help", 5).is_err());
        assert!(search_argv("   ", 5).is_err());
        assert!(find_argv("--space", None).is_err());
        assert!(find_argv("x", Some("ENG --all")).is_err());
        assert!(read_argv("12a").is_err());
        assert!(comments_argv("").is_err());
        // A query is one argument, never split, so quotes and semicolons stay inside it.
        let a = search_argv("rollout \"phase 2\"; rm -rf", 50).unwrap();
        assert_eq!(a, v(&["search", "rollout \"phase 2\"; rm -rf", "--limit", "20"]));
    }

    #[test]
    fn spots_errors_printed_as_normal_output() {
        assert!(looks_like_error("❌ Bearer or basic authentication requires a token."));
        assert!(is_auth_problem(
            "❌ Bearer or basic authentication requires a token.\nPlease rerun \"confluence init\""
        ));
        assert!(!looks_like_error("Found 3 results"));
    }

    #[test]
    fn page_title_and_tidy_comments() {
        let info = "Page Information:\nTitle: Laporan Keuangan\nID: 4566089729\nType: page";
        assert_eq!(title_from_info(info).as_deref(), Some("Laporan Keuangan"));
        assert_eq!(title_from_info("ID: 1"), None);
        let raw = "Found 2 comments:\n1. Citra (ID: 4566614048) [inline]\n   Created: 2026-09-23T08:37:43.142Z • Status: current • Version: 1 • Resolution: open\n   Highlight (original): AC1\n   Marker ref: 17b8\n   Body:\n     BE : 1\n2. Nuki (ID: 9) [footer]\n   Created: 2026-09-24T01:00:00.000Z • Status: current • Version: 2 • Resolution: resolved\n   Body:\n     done";
        assert_eq!(
            tidy_comments(raw),
            "Found 2 comments:\n1. Citra [inline]\n   2026-09-23\n   Highlight (original): AC1\n     BE : 1\n2. Nuki [footer]\n   2026-09-24 (resolved)\n     done"
        );
    }
}
