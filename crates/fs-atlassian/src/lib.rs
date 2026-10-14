//! Read-only access to Jira and Confluence for Ask.
//!
//! - Jira goes through Atlassian's official CLI, `acli`.
//! - Confluence goes through `confluence-cli` (the npm `confluence` command), because `acli`
//!   can't search Confluence. See [`confluence`].
//!
//! fivesight never writes to Atlassian. Two independent checks make sure of it:
//!
//! 1. The AI can only ask for a typed [`ReadCommand`] (search, find or read). Each maps to fixed
//!    argument lists; there is no way to name another subcommand or pass a raw string to a shell.
//! 2. Before anything runs, the final argument list is checked against an exact allowlist for
//!    that CLI: the subcommand, every flag and its value, and the number of positional values.
//!    Write commands (`create`, `edit`, `update`, `transition`, `delete`, `comment`, ...) and
//!    flags like `--web` are rejected before a process is started.
//!
//! Both CLIs run directly (no shell), without a console window, with no stdin, and with a timeout.

pub mod confluence;
mod format;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use thiserror::Error;

pub use format::{adf_to_text, format_issue, format_search};

#[derive(Debug, Error)]
pub enum AtlassianError {
    #[error("not allowed: fivesight only reads from Jira and Confluence ({0})")]
    NotAllowed(String),
    #[error("invalid {0}")]
    InvalidArg(String),
    #[error("{0} isn't installed or couldn't be found")]
    NotInstalled(&'static str),
    #[error("not logged in to Jira. Run: acli jira auth login --web")]
    JiraLogin,
    #[error("Confluence isn't set up. Run: confluence init")]
    ConfluenceLogin,
    #[error("{0} took too long to answer")]
    Timeout(&'static str),
    #[error("{0}")]
    Failed(String),
    #[error("could not start {0}: {1}")]
    Spawn(&'static str, std::io::Error),
}

/// Text handed to the AI per lookup is capped so a big ticket can't crowd out the meeting.
pub const MAX_OUTPUT_BYTES: usize = 12 * 1024;
const TIMEOUT: Duration = Duration::from_secs(25);
const SEARCH_FIELDS: &str = "key,issuetype,summary,status,assignee,priority";
const VIEW_FIELDS: &str = "key,issuetype,summary,status,assignee,reporter,priority,labels,updated,description,comment";

/// Everything fivesight can ask for. All of it is read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadCommand {
    JiraSearch {
        jql: String,
        limit: u32,
    },
    JiraView {
        key: String,
    },
    ConfluenceSearch {
        query: String,
        limit: u32,
    },
    ConfluenceFind {
        title: String,
        space: Option<String>,
    },
    /// The page as Markdown, followed by its comment threads.
    ConfluencePage {
        page_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Acli,
    ConfluenceCli,
}

impl Tool {
    fn name(self) -> &'static str {
        match self {
            Tool::Acli => "acli",
            Tool::ConfluenceCli => "confluence-cli",
        }
    }
}

fn check_text(name: &str, v: &str) -> Result<(), AtlassianError> {
    let v = v.trim();
    if v.is_empty() || v.len() > 1000 || v.contains('\0') || v.starts_with('-') {
        return Err(AtlassianError::InvalidArg(format!("{name}: {v:?}")));
    }
    Ok(())
}

/// `PAY-142`: a project key (capital letter, then capitals, digits or `_`), a dash, a number.
pub fn is_issue_key(s: &str) -> bool {
    let Some((proj, num)) = s.rsplit_once('-') else {
        return false;
    };
    proj.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && proj.len() <= 20
        && proj.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !num.is_empty()
        && num.len() <= 9
        && num.chars().all(|c| c.is_ascii_digit())
}

/// Issue keys mentioned in some text, in order, without repeats.
pub fn keys_in(text: &str, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
        let w = word.trim_matches('-');
        if is_issue_key(w) && !out.iter().any(|k| k == w) {
            out.push(w.to_string());
            if out.len() == max {
                break;
            }
        }
    }
    out
}

/// A Confluence page id from an id or a page link (`.../pages/123456/Title` or `?pageId=123456`).
pub fn page_id_from(s: &str) -> Option<String> {
    let s = s.trim();
    let digits = |t: &str| t.chars().take_while(|c| c.is_ascii_digit()).collect::<String>();
    let id = if s.chars().all(|c| c.is_ascii_digit()) {
        s.to_string()
    } else if let Some((_, rest)) = s.split_once("pageId=") {
        digits(rest)
    } else if let Some((_, rest)) = s.split_once("/pages/") {
        digits(rest)
    } else {
        String::new()
    };
    (!id.is_empty() && id.len() <= 20).then_some(id)
}

/// Confluence page links mentioned in some text, as page ids, in order, without repeats.
pub fn page_links_in(text: &str, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split_whitespace().filter(|w| w.contains("/wiki/") || w.contains("pageId=")) {
        if let Some(id) = page_id_from(word.trim_matches(|c: char| "()<>[]\"',.".contains(c))) {
            if !out.contains(&id) {
                out.push(id);
                if out.len() == max {
                    break;
                }
            }
        }
    }
    out
}

impl ReadCommand {
    fn tool(&self) -> Tool {
        match self {
            Self::JiraSearch { .. } | Self::JiraView { .. } => Tool::Acli,
            _ => Tool::ConfluenceCli,
        }
    }

    /// The fixed argument lists (without the program) this command runs, in order.
    fn plan(&self) -> Result<Vec<Vec<String>>, AtlassianError> {
        let s = |x: &str| x.to_string();
        Ok(match self {
            Self::JiraSearch { jql, limit } => {
                check_text("JQL", jql)?;
                vec![vec![
                    s("jira"),
                    s("workitem"),
                    s("search"),
                    s("--jql"),
                    jql.trim().to_string(),
                    s("--limit"),
                    (*limit).clamp(1, 20).to_string(),
                    s("--fields"),
                    s(SEARCH_FIELDS),
                    s("--json"),
                ]]
            }
            Self::JiraView { key } => {
                if !is_issue_key(key) {
                    return Err(AtlassianError::InvalidArg(format!("issue key: {key:?}")));
                }
                vec![vec![s("jira"), s("workitem"), s("view"), key.clone(), s("--fields"), s(VIEW_FIELDS), s("--json")]]
            }
            Self::ConfluenceSearch { query, limit } => vec![confluence::search_argv(query, *limit)?],
            Self::ConfluenceFind { title, space } => vec![confluence::find_argv(title, space.as_deref())?],
            Self::ConfluencePage { page_id } => {
                vec![confluence::read_argv(page_id)?, confluence::comments_argv(page_id)?]
            }
        })
    }

    /// Short description for the UI, e.g. "Read PAY-142".
    pub fn label(&self) -> String {
        let short = |t: &str| t.chars().take(60).collect::<String>();
        match self {
            Self::JiraSearch { jql, .. } => format!("Searched Jira: {}", short(jql)),
            Self::JiraView { key } => format!("Read {key}"),
            Self::ConfluenceSearch { query, .. } => format!("Searched Confluence: {}", short(query)),
            Self::ConfluenceFind { title, .. } => format!("Found Confluence page: {}", short(title)),
            Self::ConfluencePage { page_id } => format!("Read Confluence page {page_id}"),
        }
    }
}

// ---------- the acli allowlist ----------

struct Allowed {
    verbs: [&'static str; 3],
    positional: usize,
    value_flags: &'static [&'static str],
    bool_flags: &'static [&'static str],
}

/// The complete list of `acli` commands that may run. Nothing that changes data is here.
const ALLOWED: &[Allowed] = &[
    Allowed {
        verbs: ["jira", "workitem", "search"],
        positional: 0,
        value_flags: &["--jql", "--limit", "--fields"],
        bool_flags: &["--json"],
    },
    Allowed { verbs: ["jira", "workitem", "view"], positional: 1, value_flags: &["--fields"], bool_flags: &["--json"] },
    Allowed { verbs: ["jira", "auth", "status"], positional: 0, value_flags: &[], bool_flags: &[] },
];

/// Check a full `acli` argument list against the allowlist: subcommand, every flag, every value.
pub fn validate_argv(argv: &[String]) -> Result<(), AtlassianError> {
    let reject = || AtlassianError::NotAllowed(argv.join(" "));
    if argv.len() < 3 || argv.iter().any(|a| a.contains('\0')) {
        return Err(reject());
    }
    let spec = ALLOWED.iter().find(|a| a.verbs.iter().zip(argv).all(|(v, a)| v == a)).ok_or_else(reject)?;
    let mut positional = 0;
    let mut rest = argv[3..].iter();
    while let Some(tok) = rest.next() {
        if spec.value_flags.contains(&tok.as_str()) {
            match rest.next() {
                Some(v) if !v.starts_with('-') && !v.is_empty() => {}
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

// ---------- running the CLIs ----------

/// Where `acli` is: `FIVESIGHT_ACLI`, then `PATH`, then the folder Atlassian's install guide uses.
pub fn find_acli() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FIVESIGHT_ACLI") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let exe = if cfg!(windows) { "acli.exe" } else { "acli" };
    if let Some(paths) = std::env::var_os("PATH") {
        if let Some(p) = std::env::split_paths(&paths).map(|d| d.join(exe)).find(|p| p.is_file()) {
            return Some(p);
        }
    }
    let local = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Programs").join("acli").join(exe));
    local.filter(|p| p.is_file())
}

struct Output {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// Start a program with no shell, no console window and no stdin; collect its output or time out.
fn spawn(program: &Path, args: &[String], tool: Tool) -> Result<Output, AtlassianError> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| AtlassianError::Spawn(tool.name(), e))?;
    let mut out_pipe = child.stdout.take().expect("piped");
    let mut err_pipe = child.stderr.take().expect("piped");
    let out_reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out_pipe.read_to_end(&mut b);
        b
    });
    let err_reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err_pipe.read_to_end(&mut b);
        b
    });
    let started = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| AtlassianError::Spawn(tool.name(), e))? {
            break s;
        }
        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AtlassianError::Timeout(if tool == Tool::Acli { "Jira" } else { "Confluence" }));
        }
        std::thread::sleep(Duration::from_millis(40));
    };
    Ok(Output {
        ok: status.success(),
        stdout: String::from_utf8_lossy(&out_reader.join().unwrap_or_default()).into_owned(),
        stderr: String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).into_owned(),
    })
}

/// Validate an argument list for its CLI, run it, and return stdout.
fn exec(tool: Tool, argv: &[String]) -> Result<String, AtlassianError> {
    match tool {
        Tool::Acli => {
            validate_argv(argv)?;
            let bin = find_acli().ok_or(AtlassianError::NotInstalled("acli"))?;
            let out = spawn(&bin, argv, tool)?;
            if !out.ok {
                let msg = if out.stderr.trim().is_empty() { out.stdout.trim() } else { out.stderr.trim() };
                if msg.to_ascii_lowercase().contains("unauthorized") || msg.contains("auth login") {
                    return Err(AtlassianError::JiraLogin);
                }
                return Err(AtlassianError::Failed(msg.trim_start_matches(['✗', ' ']).chars().take(300).collect()));
            }
            Ok(out.stdout)
        }
        Tool::ConfluenceCli => {
            confluence::validate_argv(argv)?;
            let (node, script) = confluence::find().ok_or(AtlassianError::NotInstalled("confluence-cli"))?;
            let mut args = vec![script.to_string_lossy().into_owned()];
            args.extend(argv.iter().cloned());
            let out = spawn(&node, &args, tool)?;
            if !out.ok || confluence::looks_like_error(&out.stdout) {
                let msg = format!("{}\n{}", out.stdout.trim(), out.stderr.trim());
                if confluence::is_auth_problem(&msg) {
                    return Err(AtlassianError::ConfluenceLogin);
                }
                return Err(AtlassianError::Failed(
                    msg.trim().trim_start_matches(['❌', ' ']).chars().take(300).collect(),
                ));
            }
            Ok(out.stdout)
        }
    }
}

/// Run a read command and return compact text for the AI.
pub fn run(cmd: &ReadCommand) -> Result<String, AtlassianError> {
    let tool = cmd.tool();
    let plan = cmd.plan()?;
    let text = match cmd {
        ReadCommand::JiraSearch { .. } | ReadCommand::JiraView { .. } => {
            let raw = exec(tool, &plan[0])?;
            match serde_json::from_str::<Value>(&raw) {
                Ok(v) if matches!(cmd, ReadCommand::JiraSearch { .. }) => format_search(&v),
                Ok(v) => format_issue(&v),
                Err(_) => raw,
            }
        }
        ReadCommand::ConfluencePage { .. } => {
            let page = exec(tool, &plan[0])?;
            // Decisions often live in the comment threads; a page still reads fine without them.
            let comments = match exec(tool, &plan[1]) {
                Ok(c) if !c.trim().is_empty() => c,
                Ok(_) => "(no comments)".into(),
                Err(e) => format!("(comments unavailable: {e})"),
            };
            format!("{}\n\n## Comments\n\n{}", page.trim(), comments.trim())
        }
        ReadCommand::ConfluenceSearch { .. } | ReadCommand::ConfluenceFind { .. } => exec(tool, &plan[0])?,
    };
    Ok(truncate_output(text))
}

/// What fivesight can reach. Never fails: missing pieces are just `None`/`false`.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// `acli` is installed (used for Jira).
    pub installed: bool,
    pub jira_site: Option<String>,
    /// `confluence-cli` and Node are installed.
    pub confluence_cli: bool,
    /// `confluence-cli` is set up and answered.
    pub confluence: bool,
}

pub fn status() -> Status {
    let s = |x: &str| x.to_string();
    let installed = find_acli().is_some();
    let jira_site = if installed {
        exec(Tool::Acli, &[s("jira"), s("auth"), s("status")]).ok().and_then(|out| {
            out.lines()
                .find_map(|l| l.trim().strip_prefix("Site:").map(|v| v.trim().to_string()))
                .filter(|v| !v.is_empty())
        })
    } else {
        None
    };
    let confluence_cli = confluence::find().is_some();
    let confluence = confluence_cli && exec(Tool::ConfluenceCli, &confluence::status_argv()).is_ok();
    Status { installed, jira_site, confluence_cli, confluence }
}

/// Truncate on a char boundary and mark the cut.
pub fn truncate_output(mut s: String) -> String {
    if s.len() > MAX_OUTPUT_BYTES {
        let mut cut = MAX_OUTPUT_BYTES;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n[truncated]");
    }
    s
}

// ---------- tools offered to the AI ----------

/// OpenAI-style tool definitions for the lookups the AI may request.
pub fn tool_specs(jira: bool, confluence: bool) -> Value {
    let mut tools = Vec::new();
    if jira {
        tools.push(json!({"type": "function", "function": {
            "name": "jira_search",
            "description": "Search Jira work items with JQL. Read-only. Returns key, type, summary, status, assignee and priority for each match.",
            "parameters": {"type": "object", "properties": {
                "jql": {"type": "string", "description": "A JQL query, e.g. text ~ \"checkout timeout\" ORDER BY updated DESC"},
                "limit": {"type": "integer", "description": "How many results (1-20)", "minimum": 1, "maximum": 20}
            }, "required": ["jql"]}
        }}));
        tools.push(json!({"type": "function", "function": {
            "name": "jira_view",
            "description": "Read one Jira work item by key, including description and recent comments. Read-only.",
            "parameters": {"type": "object", "properties": {
                "key": {"type": "string", "description": "Issue key such as PAY-142"}
            }, "required": ["key"]}
        }}));
    }
    if confluence {
        tools.push(json!({"type": "function", "function": {
            "name": "confluence_search",
            "description": "Full-text search of Confluence pages. Read-only. Returns titles and page ids; read a page with confluence_page.",
            "parameters": {"type": "object", "properties": {
                "query": {"type": "string", "description": "Words to search for, e.g. checkout retry policy"},
                "limit": {"type": "integer", "description": "How many results (1-20)", "minimum": 1, "maximum": 20}
            }, "required": ["query"]}
        }}));
        tools.push(json!({"type": "function", "function": {
            "name": "confluence_find",
            "description": "Find a Confluence page by its title, optionally within one space. Read-only.",
            "parameters": {"type": "object", "properties": {
                "title": {"type": "string", "description": "The page title"},
                "space": {"type": "string", "description": "Optional space key, e.g. ENG"}
            }, "required": ["title"]}
        }}));
        tools.push(json!({"type": "function", "function": {
            "name": "confluence_page",
            "description": "Read a Confluence page as Markdown, followed by its comment threads (inline, footer and resolved), where decisions are often made. Read-only.",
            "parameters": {"type": "object", "properties": {
                "page": {"type": "string", "description": "Page id (digits) or a Confluence page link"}
            }, "required": ["page"]}
        }}));
    }
    Value::Array(tools)
}

/// Turn a tool call from the AI into a command, or explain why not.
pub fn from_tool_call(name: &str, arguments: &str) -> Result<ReadCommand, AtlassianError> {
    let args: Value = serde_json::from_str(if arguments.trim().is_empty() { "{}" } else { arguments })
        .map_err(|_| AtlassianError::InvalidArg(format!("arguments for {name}")))?;
    let text = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let limit = || args.get("limit").and_then(Value::as_u64).unwrap_or(10).min(20) as u32;
    match name {
        "jira_search" => Ok(ReadCommand::JiraSearch { jql: text("jql"), limit: limit() }),
        "jira_view" => Ok(ReadCommand::JiraView { key: text("key").to_ascii_uppercase() }),
        "confluence_search" => Ok(ReadCommand::ConfluenceSearch { query: text("query"), limit: limit() }),
        "confluence_find" => Ok(ReadCommand::ConfluenceFind {
            title: text("title"),
            space: Some(text("space")).filter(|s| !s.is_empty()),
        }),
        "confluence_page" => page_id_from(&text("page"))
            .map(|page_id| ReadCommand::ConfluencePage { page_id })
            .ok_or_else(|| AtlassianError::InvalidArg("Confluence page id or link".into())),
        other => Err(AtlassianError::NotAllowed(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn validate(cmd: &ReadCommand) -> Result<(), AtlassianError> {
        for argv in cmd.plan()? {
            match cmd.tool() {
                Tool::Acli => validate_argv(&argv)?,
                Tool::ConfluenceCli => confluence::validate_argv(&argv)?,
            }
        }
        Ok(())
    }

    #[test]
    fn every_command_fivesight_builds_passes_its_allowlist() {
        for cmd in [
            ReadCommand::JiraSearch { jql: "project = PAY".into(), limit: 5 },
            ReadCommand::JiraView { key: "PAY-142".into() },
            ReadCommand::ConfluenceSearch { query: "retry policy".into(), limit: 5 },
            ReadCommand::ConfluenceFind { title: "Payments runbook".into(), space: Some("ENG".into()) },
            ReadCommand::ConfluencePage { page_id: "123456".into() },
        ] {
            validate(&cmd).unwrap();
        }
    }

    #[test]
    fn write_commands_are_rejected() {
        // Every write-capable acli 1.3 subcommand, plus Confluence through acli, which isn't used.
        for bad in [
            v(&["jira", "workitem", "create", "--summary", "x"]),
            v(&["jira", "workitem", "create-bulk"]),
            v(&["jira", "workitem", "edit", "PAY-1"]),
            v(&["jira", "workitem", "transition", "PAY-1"]),
            v(&["jira", "workitem", "assign", "PAY-1"]),
            v(&["jira", "workitem", "delete", "PAY-1"]),
            v(&["jira", "workitem", "archive", "PAY-1"]),
            v(&["jira", "workitem", "unarchive", "PAY-1"]),
            v(&["jira", "workitem", "clone", "PAY-1"]),
            v(&["jira", "workitem", "comment", "create"]),
            v(&["jira", "workitem", "link", "create"]),
            v(&["jira", "workitem", "watcher", "add"]),
            v(&["jira", "workitem", "attachment", "delete"]),
            v(&["jira", "sprint", "create"]),
            v(&["jira", "project", "create"]),
            v(&["confluence", "page", "view", "--id", "1"]),
            v(&["confluence", "page", "create"]),
            v(&["confluence", "space", "update"]),
            v(&["jira", "auth", "login"]),
            v(&["jira", "auth", "logout"]),
            v(&["admin", "user", "delete"]),
            v(&["rovodev", "run"]),
            v(&["config", "set"]),
            v(&[]),
            v(&["jira"]),
        ] {
            assert!(matches!(validate_argv(&bad), Err(AtlassianError::NotAllowed(_))), "{bad:?}");
        }
    }

    #[test]
    fn unexpected_flags_and_values_are_rejected() {
        for bad in [
            // Opening a browser, paginating everything, or any flag not on the list.
            v(&["jira", "workitem", "search", "--jql", "x", "--web"]),
            v(&["jira", "workitem", "search", "--jql", "x", "-w"]),
            v(&["jira", "workitem", "search", "--jql", "x", "--paginate"]),
            v(&["jira", "workitem", "view", "PAY-1", "--web"]),
            v(&["jira", "workitem", "search", "--jql=x"]),
            // A flag where a value belongs, or a missing value.
            v(&["jira", "workitem", "search", "--jql", "--web"]),
            v(&["jira", "workitem", "search", "--jql"]),
            // Extra positional values.
            v(&["jira", "workitem", "view", "PAY-1", "PAY-2"]),
            v(&["jira", "workitem", "search", "stray"]),
            // A missing key.
            v(&["jira", "workitem", "view", "--json"]),
            v(&["jira", "workitem", "view", "PAY-1\0"]),
        ] {
            assert!(validate_argv(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn inputs_from_the_ai_cannot_smuggle_flags() {
        assert!(ReadCommand::JiraSearch { jql: "--web".into(), limit: 5 }.plan().is_err());
        assert!(ReadCommand::JiraSearch { jql: "  ".into(), limit: 5 }.plan().is_err());
        assert!(ReadCommand::JiraView { key: "PAY-1 --web".into() }.plan().is_err());
        assert!(ReadCommand::JiraView { key: "--web".into() }.plan().is_err());
        assert!(ReadCommand::ConfluencePage { page_id: "12a".into() }.plan().is_err());
        assert!(ReadCommand::ConfluenceSearch { query: "--cql".into(), limit: 5 }.plan().is_err());
        assert!(from_tool_call("jira_create", "{}").is_err());
        assert!(from_tool_call("confluence_update", "{}").is_err());
        assert!(from_tool_call("jira_view", "not json").is_err());
        // JQL is passed as one argument, never split, so quotes and semicolons stay inside it.
        let plan = ReadCommand::JiraSearch { jql: "text ~ \"a; rm -rf\"".into(), limit: 5 }.plan().unwrap();
        assert_eq!(plan[0][4], "text ~ \"a; rm -rf\"");
        validate_argv(&plan[0]).unwrap();
    }

    #[test]
    fn tool_calls_map_to_commands() {
        assert_eq!(
            from_tool_call("jira_search", r#"{"jql":"project = PAY","limit":99}"#).unwrap(),
            ReadCommand::JiraSearch { jql: "project = PAY".into(), limit: 20 }
        );
        assert_eq!(
            from_tool_call("jira_view", r#"{"key":"pay-142"}"#).unwrap(),
            ReadCommand::JiraView { key: "PAY-142".into() }
        );
        assert_eq!(
            from_tool_call("confluence_search", r#"{"query":"retry policy"}"#).unwrap(),
            ReadCommand::ConfluenceSearch { query: "retry policy".into(), limit: 10 }
        );
        assert_eq!(
            from_tool_call("confluence_find", r#"{"title":"Runbook","space":""}"#).unwrap(),
            ReadCommand::ConfluenceFind { title: "Runbook".into(), space: None }
        );
        assert_eq!(
            from_tool_call(
                "confluence_page",
                r#"{"page":"https://x.atlassian.net/wiki/spaces/ENG/pages/98765/Runbook"}"#
            )
            .unwrap(),
            ReadCommand::ConfluencePage { page_id: "98765".into() }
        );
        let names = |t: Value| -> Vec<String> {
            t.as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap().to_string()).collect()
        };
        assert_eq!(
            names(tool_specs(true, true)),
            ["jira_search", "jira_view", "confluence_search", "confluence_find", "confluence_page"]
        );
        assert_eq!(names(tool_specs(true, false)), ["jira_search", "jira_view"]);
        assert_eq!(names(tool_specs(false, true)), ["confluence_search", "confluence_find", "confluence_page"]);
    }

    #[test]
    fn limit_is_clamped() {
        let a = ReadCommand::JiraSearch { jql: "project = X".into(), limit: 9999 }.plan().unwrap();
        assert!(a[0].contains(&"20".to_string()));
        let a = ReadCommand::JiraSearch { jql: "project = X".into(), limit: 0 }.plan().unwrap();
        assert!(a[0].contains(&"1".to_string()));
    }

    #[test]
    fn finds_issue_keys_and_page_links() {
        assert_eq!(keys_in("Is PAY-142 related to (OPS-7)? PAY-142 again, not pay-1 or PAY-", 5), ["PAY-142", "OPS-7"]);
        assert_eq!(keys_in("A-1 B-2 C-3", 2), ["A-1", "B-2"]);
        assert!(is_issue_key("A1_B-9"));
        for bad in ["abc-1", "ABC-", "ABC", "-1", "ABC-1; rm", "ABC-1 --x"] {
            assert!(!is_issue_key(bad), "{bad}");
        }
        assert_eq!(page_id_from("123456").as_deref(), Some("123456"));
        assert_eq!(page_id_from("https://x/wiki/pages/viewpage.action?pageId=777").as_deref(), Some("777"));
        assert_eq!(page_id_from("not a page"), None);
        assert_eq!(
            page_links_in(
                "see (https://x.atlassian.net/wiki/spaces/ENG/pages/98765/Runbook), and 12345 isn't a link",
                3
            ),
            ["98765"]
        );
    }

    #[test]
    fn truncates_on_char_boundary() {
        let t = truncate_output("é".repeat(MAX_OUTPUT_BYTES));
        assert!(t.ends_with("[truncated]"));
        assert!(t.len() <= MAX_OUTPUT_BYTES + 20);
    }
}
