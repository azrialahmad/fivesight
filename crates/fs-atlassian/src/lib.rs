//! Read-only wrapper around the Atlassian CLI (`acli`).
//!
//! The LLM never builds a shell string. It supplies a typed [`ReadCommand`];
//! this module maps it to a fixed argv and runs it with no shell involved.
//! Anything not in the allowlist is rejected before a process is spawned.

use std::process::Command;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AtlassianError {
    #[error("command not allowed (fivesight is read-only): {0}")]
    NotAllowed(String),
    #[error("invalid argument: {0}")]
    InvalidArg(String),
    #[error("acli failed: {0}")]
    Failed(String),
    #[error("could not run acli: {0}")]
    Spawn(#[from] std::io::Error),
}

/// Maximum bytes of acli output handed back to the LLM.
pub const MAX_OUTPUT_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadCommand {
    JiraSearch { jql: String, limit: u32 },
    JiraView { key: String },
    ConfluenceSearch { cql: String, limit: u32 },
    ConfluenceView { page_id: String },
}

fn check_arg(name: &str, v: &str) -> Result<(), AtlassianError> {
    if v.is_empty() || v.len() > 1000 || v.contains('\0') || v.starts_with('-') {
        return Err(AtlassianError::InvalidArg(format!("{name}: {v:?}")));
    }
    Ok(())
}

fn is_issue_key(s: &str) -> bool {
    let Some((proj, num)) = s.rsplit_once('-') else {
        return false;
    };
    !proj.is_empty()
        && proj.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && proj
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !num.is_empty()
        && num.chars().all(|c| c.is_ascii_digit())
}

impl ReadCommand {
    /// The fixed argv (excluding the program name) for this command.
    pub fn argv(&self) -> Result<Vec<String>, AtlassianError> {
        let s = |x: &str| x.to_string();
        Ok(match self {
            Self::JiraSearch { jql, limit } => {
                check_arg("jql", jql)?;
                vec![
                    s("jira"),
                    s("workitem"),
                    s("search"),
                    s("--jql"),
                    jql.clone(),
                    s("--limit"),
                    (*limit).clamp(1, 50).to_string(),
                    s("--json"),
                ]
            }
            Self::JiraView { key } => {
                if !is_issue_key(key) {
                    return Err(AtlassianError::InvalidArg(format!("issue key: {key:?}")));
                }
                vec![s("jira"), s("workitem"), s("view"), key.clone(), s("--json")]
            }
            Self::ConfluenceSearch { cql, limit } => {
                check_arg("cql", cql)?;
                vec![
                    s("confluence"),
                    s("page"),
                    s("search"),
                    s("--cql"),
                    cql.clone(),
                    s("--limit"),
                    (*limit).clamp(1, 50).to_string(),
                    s("--json"),
                ]
            }
            Self::ConfluenceView { page_id } => {
                if page_id.is_empty() || !page_id.chars().all(|c| c.is_ascii_digit()) {
                    return Err(AtlassianError::InvalidArg(format!("page id: {page_id:?}")));
                }
                vec![
                    s("confluence"),
                    s("page"),
                    s("view"),
                    page_id.clone(),
                    s("--json"),
                ]
            }
        })
    }
}

/// Validate a raw argv against the read-only allowlist. Second line of defence.
pub fn validate_argv(argv: &[String]) -> Result<(), AtlassianError> {
    let verbs: Vec<&str> = argv.iter().take(3).map(String::as_str).collect();
    match verbs.as_slice() {
        ["jira", "workitem", "search" | "view"] | ["confluence", "page", "search" | "view"] => Ok(()),
        _ => Err(AtlassianError::NotAllowed(argv.join(" "))),
    }
}

/// Truncate on a char boundary and mark the cut.
pub fn truncate_output(mut s: String) -> String {
    if s.len() > MAX_OUTPUT_BYTES {
        let mut cut = MAX_OUTPUT_BYTES;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n[output truncated]");
    }
    s
}

/// Run a read command. Direct exec, no shell.
pub fn run(cmd: &ReadCommand) -> Result<String, AtlassianError> {
    let argv = cmd.argv()?;
    validate_argv(&argv)?;
    let out = Command::new("acli").args(&argv).output()?;
    if !out.status.success() {
        return Err(AtlassianError::Failed(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(truncate_output(
        String::from_utf8_lossy(&out.stdout).into_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn allows_read_verbs() {
        assert!(validate_argv(&v(&["jira", "workitem", "search", "--jql", "x"])).is_ok());
        assert!(validate_argv(&v(&["confluence", "page", "view", "123"])).is_ok());
    }

    #[test]
    fn rejects_write_verbs() {
        for bad in [
            v(&["jira", "workitem", "create", "--summary", "x"]),
            v(&["jira", "workitem", "edit", "ABC-1"]),
            v(&["jira", "workitem", "transition", "ABC-1"]),
            v(&["jira", "workitem", "delete", "ABC-1"]),
            v(&["confluence", "page", "create"]),
            v(&["auth", "login"]),
            v(&[]),
        ] {
            assert!(
                matches!(validate_argv(&bad), Err(AtlassianError::NotAllowed(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn issue_key_validation() {
        assert!(is_issue_key("ABC-123"));
        assert!(is_issue_key("A1_B-9"));
        for bad in ["abc-1", "ABC-", "ABC", "-1", "ABC-1; rm", "ABC-1 --x"] {
            assert!(!is_issue_key(bad), "{bad}");
        }
        assert!(ReadCommand::JiraView {
            key: "ABC-1 --x".into()
        }
        .argv()
        .is_err());
    }

    #[test]
    fn flag_injection_blocked() {
        assert!(ReadCommand::JiraSearch {
            jql: "--help".into(),
            limit: 5
        }
        .argv()
        .is_err());
        assert!(ReadCommand::ConfluenceView {
            page_id: "12a".into()
        }
        .argv()
        .is_err());
    }

    #[test]
    fn limit_is_clamped() {
        let a = ReadCommand::JiraSearch {
            jql: "project = X".into(),
            limit: 9999,
        }
        .argv()
        .unwrap();
        assert!(a.contains(&"50".to_string()));
    }

    #[test]
    fn truncates_on_char_boundary() {
        let s = "é".repeat(MAX_OUTPUT_BYTES);
        let t = truncate_output(s);
        assert!(t.ends_with("[output truncated]"));
    }
}
