//! Turn `acli`'s JSON into short plain text. Raw work-item JSON is ~3 KB of mostly empty
//! metadata per ticket; the AI only needs the fields people actually read.

use serde_json::Value;

fn s<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for p in path {
        cur = cur.get(p)?;
    }
    cur.as_str().filter(|x| !x.trim().is_empty())
}

/// Atlassian Document Format (the JSON rich text Jira uses) to plain text.
pub fn adf_to_text(v: &Value) -> String {
    fn walk(v: &Value, out: &mut String) {
        match v {
            Value::String(t) => out.push_str(t),
            Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            Value::Object(o) => {
                let kind = o.get("type").and_then(Value::as_str).unwrap_or("");
                match kind {
                    "text" => out.push_str(o.get("text").and_then(Value::as_str).unwrap_or("")),
                    "hardBreak" => out.push('\n'),
                    "mention" | "emoji" | "status" | "date" => {
                        let attrs = o.get("attrs");
                        let t = attrs
                            .and_then(|a| a.get("text").or_else(|| a.get("shortName")).or_else(|| a.get("timestamp")))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        out.push_str(t);
                    }
                    "inlineCard" | "blockCard" => {
                        if let Some(url) = o.get("attrs").and_then(|a| a.get("url")).and_then(Value::as_str) {
                            out.push_str(url);
                        }
                    }
                    _ => {
                        if kind == "listItem" {
                            out.push_str("- ");
                        }
                        if let Some(c) = o.get("content") {
                            walk(c, out);
                        }
                        if matches!(
                            kind,
                            "paragraph"
                                | "heading"
                                | "listItem"
                                | "codeBlock"
                                | "blockquote"
                                | "tableRow"
                                | "rule"
                                | "panel"
                        ) {
                            out.push('\n');
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    walk(v, &mut out);
    tidy(&out)
}

/// Collapse runs of blank lines and trailing spaces.
fn tidy(t: &str) -> String {
    let mut out = String::new();
    let mut blank = 0;
    for line in t.lines().map(str::trim_end) {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

/// Strip HTML to text, keeping line breaks at block elements. Enough for Confluence page bodies.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut tag = String::new();
    let mut in_tag = false;
    let mut skip = false; // inside <script> or <style>
    for c in html.chars() {
        match (in_tag, c) {
            (false, '<') => {
                in_tag = true;
                tag.clear();
            }
            (true, '>') => {
                in_tag = false;
                let name: String = tag
                    .trim_start_matches('/')
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect::<String>()
                    .to_ascii_lowercase();
                if name == "script" || name == "style" {
                    skip = !tag.starts_with('/');
                }
                let closing = tag.starts_with('/');
                let block = matches!(
                    name.as_str(),
                    "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "pre" | "table"
                );
                // Break after a block ends (and at <br>), not at both ends, to avoid blank lines.
                if (block && closing) || name == "br" {
                    out.push('\n');
                }
                if name == "li" && !tag.starts_with('/') {
                    out.push_str("- ");
                }
                if (name == "td" || name == "th") && tag.starts_with('/') {
                    out.push_str(" | ");
                }
            }
            (true, c) => tag.push(c),
            (false, c) if !skip => out.push(c),
            _ => {}
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&rsquo;", "'")
        .replace("&ldquo;", "\"")
        .replace("&rdquo;", "\"")
        .replace("&amp;", "&");
    tidy(&decoded)
}

fn issue_line(i: &Value) -> String {
    let f = i.get("fields").unwrap_or(&Value::Null);
    let key = s(i, &["key"]).unwrap_or("?");
    let mut line = format!("{key}: {}", s(f, &["summary"]).unwrap_or("(no summary)"));
    let mut meta = Vec::new();
    if let Some(t) = s(f, &["issuetype", "name"]) {
        meta.push(t.to_string());
    }
    if let Some(t) = s(f, &["status", "name"]) {
        meta.push(format!("status {t}"));
    }
    meta.push(format!("assignee {}", s(f, &["assignee", "displayName"]).unwrap_or("unassigned")));
    if let Some(t) = s(f, &["priority", "name"]) {
        meta.push(format!("priority {t}"));
    }
    line.push_str(&format!(" ({})", meta.join(", ")));
    line
}

/// A search result list, one work item per line.
pub fn format_search(v: &Value) -> String {
    let items = v.as_array().or_else(|| v.get("issues").and_then(Value::as_array));
    match items {
        Some(items) if !items.is_empty() => items.iter().map(issue_line).collect::<Vec<_>>().join("\n"),
        _ => "No work items matched.".into(),
    }
}

/// One work item with its description and the latest comments.
pub fn format_issue(v: &Value) -> String {
    let f = v.get("fields").unwrap_or(&Value::Null);
    let mut out = issue_line(v);
    if let Some(r) = s(f, &["reporter", "displayName"]) {
        out.push_str(&format!("\nReporter: {r}"));
    }
    if let Some(labels) = f.get("labels").and_then(Value::as_array).filter(|l| !l.is_empty()) {
        let l: Vec<&str> = labels.iter().filter_map(Value::as_str).collect();
        out.push_str(&format!("\nLabels: {}", l.join(", ")));
    }
    if let Some(u) = s(f, &["updated"]) {
        out.push_str(&format!("\nUpdated: {}", u.chars().take(10).collect::<String>()));
    }
    let desc = match f.get("description") {
        Some(Value::String(t)) => t.trim().to_string(),
        Some(d @ Value::Object(_)) => adf_to_text(d),
        _ => String::new(),
    };
    out.push_str("\n\nDescription:\n");
    out.push_str(if desc.is_empty() { "(none)" } else { &desc });

    let comments = f.get("comment").and_then(|c| c.get("comments")).and_then(Value::as_array);
    if let Some(cs) = comments.filter(|c| !c.is_empty()) {
        const SHOWN: usize = 5;
        out.push_str(&format!("\n\nLatest comments ({} of {}):", cs.len().min(SHOWN), cs.len()));
        for c in cs.iter().rev().take(SHOWN).collect::<Vec<_>>().into_iter().rev() {
            let who = s(c, &["author", "displayName"]).unwrap_or("someone");
            let when = s(c, &["created"]).map(|d| d.chars().take(10).collect::<String>()).unwrap_or_default();
            let body = match c.get("body") {
                Some(Value::String(t)) => t.trim().to_string(),
                Some(b) => adf_to_text(b),
                None => String::new(),
            };
            out.push_str(&format!("\n- {who} ({when}): {body}"));
        }
    }
    out
}

/// A Confluence page: title, then the body as text.
pub fn format_page(v: &Value) -> String {
    let title = s(v, &["title"]).unwrap_or("(untitled page)");
    let body = s(v, &["body", "view", "value"])
        .or_else(|| s(v, &["body", "storage", "value"]))
        .or_else(|| s(v, &["body", "export_view", "value"]))
        .map(html_to_text)
        .unwrap_or_default();
    let mut out = format!("Confluence page: {title}");
    if let Some(space) = s(v, &["spaceId"]) {
        out.push_str(&format!(" (space {space})"));
    }
    out.push_str("\n\n");
    out.push_str(if body.is_empty() { "(no readable body)" } else { &body });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn adf_becomes_readable_text() {
        let adf = json!({"type": "doc", "version": 1, "content": [
            {"type": "heading", "content": [{"type": "text", "text": "Root cause"}]},
            {"type": "paragraph", "content": [
                {"type": "text", "text": "Retry waits "}, {"type": "text", "text": "30 s", "marks": [{"type": "strong"}]},
                {"type": "text", "text": ", ask "}, {"type": "mention", "attrs": {"text": "@Budi"}}
            ]},
            {"type": "bulletList", "content": [
                {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "fix window"}]}]},
                {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "add test"}]}]}
            ]}
        ]});
        let t = adf_to_text(&adf);
        assert!(t.starts_with("Root cause\nRetry waits 30 s, ask @Budi"), "{t}");
        assert!(t.contains("- fix window") && t.contains("- add test"), "{t}");
    }

    #[test]
    fn search_results_are_one_line_each() {
        let v = json!([
            {"key": "PAY-142", "fields": {"summary": "Checkout times out", "status": {"name": "In Progress"},
              "assignee": {"displayName": "Budi"}, "priority": {"name": "High"}, "issuetype": {"name": "Bug"}}},
            {"key": "PAY-7", "fields": {"summary": "Retry config", "status": {"name": "Done"}, "assignee": null}}
        ]);
        let t = format_search(&v);
        assert_eq!(
            t,
            "PAY-142: Checkout times out (Bug, status In Progress, assignee Budi, priority High)\n\
             PAY-7: Retry config (status Done, assignee unassigned)"
        );
        assert_eq!(format_search(&json!([])), "No work items matched.");
    }

    #[test]
    fn an_issue_shows_description_and_latest_comments() {
        let comments: Vec<_> = (1..=7)
            .map(|i| json!({"author": {"displayName": format!("P{i}")}, "created": "2026-10-01T10:00:00.000+0700",
                "body": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": format!("note {i}")}]}]}}))
            .collect();
        let v = json!({"key": "PAY-142", "fields": {
            "summary": "Checkout times out", "status": {"name": "Open"}, "reporter": {"displayName": "Sari"},
            "labels": ["checkout"], "description": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Fails after 30 s"}]}]},
            "comment": {"comments": comments}
        }});
        let t = format_issue(&v);
        assert!(
            t.contains("Reporter: Sari") && t.contains("Labels: checkout") && t.contains("Fails after 30 s"),
            "{t}"
        );
        assert!(t.contains("Latest comments (5 of 7)"), "{t}");
        assert!(!t.contains("note 2") && t.contains("note 3") && t.contains("note 7"), "{t}");
        let empty = format_issue(&json!({"key": "X-1", "fields": {"summary": "s", "description": null}}));
        assert!(empty.contains("Description:\n(none)"));
    }

    #[test]
    fn confluence_html_becomes_text() {
        let v = json!({"title": "Payments runbook", "body": {"view": {"value":
            "<h1>Retries</h1><p>Wait&nbsp;30&nbsp;s &amp; then fail.</p><ul><li>Page on-call</li><li>Post in #incidents</li></ul><script>x()</script><table><tr><td>a</td><td>b</td></tr></table>"}}});
        let t = format_page(&v);
        assert!(t.starts_with("Confluence page: Payments runbook"), "{t}");
        assert!(t.contains("Retries\nWait 30 s & then fail."), "{t}");
        assert!(t.contains("- Page on-call") && t.contains("- Post in #incidents"), "{t}");
        assert!(!t.contains("x()") && !t.contains('<'), "{t}");
        assert!(t.contains("a | b |"), "{t}");
    }
}
