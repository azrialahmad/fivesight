//! Confluence "storage format" (the XHTML a page is saved as) to plain Markdown-ish text.
//!
//! `confluence-cli`'s own Markdown flattens everything inside a table cell onto one line and
//! drops macros, so a PRD written as a requirements table (user story, then acceptance criteria
//! as nested lists) loses its structure and its Jira links. Here:
//!
//! - Tables with long or multi-line cells become one block per row, each cell under its header,
//!   with lists kept. Small tables stay Markdown tables.
//! - Jira macros become the issue key, status macros `[STATUS]`, code macros fenced code,
//!   panels and expands their body. Images become `[image]`.
//!
//! The parser is deliberately forgiving: storage format isn't strict XML (HTML entities,
//! undeclared `ac:`/`ri:` prefixes), and nothing here should ever fail on a strange page.

#[derive(Debug)]
enum Node {
    El {
        name: String,
        attrs: Vec<(String, String)>,
        kids: Vec<Node>,
    },
    Text(String),
    /// CDATA, kept verbatim (code macro bodies).
    Raw(String),
}

impl Node {
    fn attr(&self, key: &str) -> Option<&str> {
        match self {
            Node::El { attrs, .. } => attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()),
            _ => None,
        }
    }
    fn name(&self) -> &str {
        match self {
            Node::El { name, .. } => name,
            _ => "",
        }
    }
    fn kids(&self) -> &[Node] {
        match self {
            Node::El { kids, .. } => kids,
            _ => &[],
        }
    }
    fn child(&self, name: &str) -> Option<&Node> {
        self.kids().iter().find(|k| k.name() == name)
    }
    /// The `ac:parameter` of a macro with this `ac:name`.
    fn param(&self, key: &str) -> Option<String> {
        self.kids()
            .iter()
            .find(|k| k.name() == "ac:parameter" && k.attr("ac:name") == Some(key))
            .map(|p| plain(p.kids()).trim().to_string())
    }
}

const VOID: &[&str] = &["br", "hr", "img", "col", "input", "meta", "link", "wbr"];

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[1..].find(|c: char| c == ';' || c == '&' || c.is_whitespace()).map(|e| e + 1);
        let decoded = match end.filter(|&e| rest.as_bytes().get(e) == Some(&b';')) {
            Some(e) => {
                let name = &rest[1..e];
                let ch = if let Some(num) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                    u32::from_str_radix(num, 16).ok().and_then(char::from_u32).map(String::from)
                } else if let Some(num) = name.strip_prefix('#') {
                    num.parse::<u32>().ok().and_then(char::from_u32).map(String::from)
                } else {
                    named_entity(name).map(String::from)
                };
                ch.map(|c| (c, e + 1))
            }
            None => None,
        };
        match decoded {
            Some((c, len)) => {
                out.push_str(&c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn named_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => " ",
        "rsquo" | "lsquo" => "'",
        "rdquo" | "ldquo" => "\"",
        "ndash" => "–",
        "mdash" => "—",
        "hellip" => "…",
        "bull" => "•",
        "middot" => "·",
        "rarr" => "→",
        "larr" => "←",
        "times" => "×",
        "minus" => "−",
        "le" => "≤",
        "ge" => "≥",
        "deg" => "°",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "euro" => "€",
        "zwj" | "zwnj" | "shy" => "",
        _ => return None,
    })
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        let start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'/' {
            i += 1;
        }
        if start == i {
            break;
        }
        let key = s[start..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut val = String::new();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                i += 1;
                let vs = i;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                val = decode_entities(&s[vs..i]);
                i += 1;
            } else {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() {
                    i += 1;
                }
                val = decode_entities(&s[vs..i]);
            }
        }
        out.push((key, val));
    }
    out
}

/// An element still open while parsing: name, attributes, children so far.
type Open = (String, Vec<(String, String)>, Vec<Node>);

fn parse(src: &str) -> Vec<Node> {
    // A stack of open elements; index 0 is the document root.
    let mut stack: Vec<Open> = vec![(String::new(), Vec::new(), Vec::new())];
    let close = |stack: &mut Vec<Open>| {
        let (name, attrs, kids) = stack.pop().expect("not the root");
        stack.last_mut().expect("root").2.push(Node::El { name, attrs, kids });
    };
    let mut rest = src;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            stack.last_mut().expect("root").2.push(Node::Text(decode_entities(rest)));
            break;
        };
        if lt > 0 {
            stack.last_mut().expect("root").2.push(Node::Text(decode_entities(&rest[..lt])));
        }
        rest = &rest[lt..];
        if let Some(body) = rest.strip_prefix("<![CDATA[") {
            let end = body.find("]]>").unwrap_or(body.len());
            stack.last_mut().expect("root").2.push(Node::Raw(body[..end].to_string()));
            rest = body.get(end + 3..).unwrap_or("");
            continue;
        }
        if let Some(body) = rest.strip_prefix("<!--") {
            rest = body.find("-->").map_or("", |e| &body[e + 3..]);
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            rest = rest.find('>').map_or("", |e| &rest[e + 1..]);
            continue;
        }
        let Some(gt) = rest.find('>') else {
            // A stray '<' with no tag after it is just text.
            stack.last_mut().expect("root").2.push(Node::Text(decode_entities(rest)));
            break;
        };
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        if let Some(name) = tag.strip_prefix('/') {
            let name = name.trim().to_ascii_lowercase();
            // Close up to the matching open element; ignore a closing tag with no opener.
            if let Some(pos) = stack.iter().rposition(|(n, _, _)| *n == name).filter(|&p| p > 0) {
                while stack.len() > pos {
                    close(&mut stack);
                }
            }
            continue;
        }
        let self_closing = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let (name, attrs) = match tag.find(|c: char| c.is_whitespace()) {
            Some(sp) => (&tag[..sp], parse_attrs(&tag[sp..])),
            None => (tag, Vec::new()),
        };
        let name = name.to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        if self_closing || VOID.contains(&name.as_str()) {
            stack.last_mut().expect("root").2.push(Node::El { name, attrs, kids: Vec::new() });
        } else {
            stack.push((name, attrs, Vec::new()));
        }
    }
    while stack.len() > 1 {
        close(&mut stack);
    }
    stack.pop().map(|(_, _, kids)| kids).unwrap_or_default()
}

/// Text only, whitespace collapsed (for macro parameters and link labels).
fn plain(nodes: &[Node]) -> String {
    let mut out = String::new();
    for n in nodes {
        match n {
            Node::Text(t) | Node::Raw(t) => out.push_str(t),
            Node::El { kids, .. } => out.push_str(&plain(kids)),
        }
    }
    collapse(&out)
}

fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space = true;
        } else {
            // Leading space is kept too: " and " between two links must stay apart.
            if space {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
    }
    if space && !out.is_empty() {
        out.push(' ');
    }
    out
}

/// Builds the output: block elements end lines, inline text is joined with single spaces.
#[derive(Default)]
struct Out {
    lines: Vec<String>,
    cur: String,
}

impl Out {
    fn text(&mut self, t: &str) {
        let t = collapse(t);
        if t.is_empty() || (t == " " && (self.cur.is_empty() || self.cur.ends_with(' '))) {
            return;
        }
        if self.cur.ends_with(' ') && t.starts_with(' ') {
            self.cur.push_str(&t[1..]);
        } else if self.cur.is_empty() {
            self.cur.push_str(t.trim_start());
        } else {
            self.cur.push_str(&t);
        }
    }
    fn word(&mut self, w: &str) {
        if !self.cur.is_empty() && !self.cur.ends_with(' ') {
            self.cur.push(' ');
        }
        self.cur.push_str(w);
    }
    fn end_line(&mut self) {
        let line = self.cur.trim_end().to_string();
        self.cur.clear();
        if !line.trim().is_empty() {
            self.lines.push(line);
        }
    }
    fn blank(&mut self) {
        self.end_line();
        if self.lines.last().is_some_and(|l| !l.is_empty()) {
            self.lines.push(String::new());
        }
    }
    fn finish(mut self) -> Vec<String> {
        self.end_line();
        while self.lines.last().is_some_and(|l| l.is_empty()) {
            self.lines.pop();
        }
        self.lines
    }
}

fn render(nodes: &[Node]) -> Vec<String> {
    let mut out = Out::default();
    for n in nodes {
        node(n, &mut out);
    }
    out.finish()
}

fn node(n: &Node, out: &mut Out) {
    let (name, kids) = match n {
        Node::Text(t) | Node::Raw(t) => return out.text(t),
        Node::El { name, kids, .. } => (name.as_str(), kids.as_slice()),
    };
    match name {
        "p" | "div" | "section" => {
            out.end_line();
            kids.iter().for_each(|k| node(k, out));
            out.blank();
        }
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            out.blank();
            let level: usize = name[1..].parse().unwrap_or(2);
            let title = plain(kids);
            if !title.trim().is_empty() {
                out.lines.push(format!("{} {}", "#".repeat(level), title.trim()));
            }
            out.blank();
        }
        "br" => out.end_line(),
        "hr" => {
            out.blank();
            out.lines.push("---".into());
            out.blank();
        }
        "strong" | "b" => {
            let t = plain(kids);
            if !t.trim().is_empty() {
                out.word(&format!("**{}**", t.trim()));
            }
        }
        "code" => {
            let t = plain(kids);
            if !t.trim().is_empty() {
                out.word(&format!("`{}`", t.trim()));
            }
        }
        "a" => {
            let label = plain(kids);
            let label = label.trim();
            match n.attr("href").filter(|h| !h.is_empty()) {
                Some(href) if label.is_empty() || label == href => out.word(href),
                Some(href) => out.word(&format!("{label} ({href})")),
                None => out.text(label),
            }
        }
        "ul" | "ol" | "ac:task-list" => list(n, out),
        "table" => table(n, out),
        "blockquote" => {
            out.end_line();
            for l in render(kids) {
                out.lines.push(if l.is_empty() { ">".into() } else { format!("> {l}") });
            }
            out.blank();
        }
        "pre" => code_block(&plain_keep(kids), None, out),
        "time" => out.word(n.attr("datetime").unwrap_or("")),
        "img" | "ac:image" => out.word("[image]"),
        "ac:emoticon" | "ac:placeholder" | "colgroup" | "col" | "ac:parameter" => {}
        "ac:link" => link(n, out),
        "ri:user" => out.word("@someone"),
        "ac:structured-macro" | "ac:macro" => macro_(n, out),
        "ac:adf-extension" | "ac:adf-node" => {
            // Newer editor nodes (decision lists, panels). Their fallback is usually readable.
            if let Some(fb) = n.child("ac:adf-fallback") {
                kids_of(fb, out)
            } else {
                kids.iter().filter(|k| k.name() != "ac:adf-attribute").for_each(|k| node(k, out))
            }
        }
        "ac:adf-attribute" => {}
        _ => kids.iter().for_each(|k| node(k, out)),
    }
}

fn kids_of(n: &Node, out: &mut Out) {
    n.kids().iter().for_each(|k| node(k, out));
}

/// Text with line breaks kept (code).
fn plain_keep(nodes: &[Node]) -> String {
    let mut out = String::new();
    for n in nodes {
        match n {
            Node::Text(t) | Node::Raw(t) => out.push_str(t),
            Node::El { name, kids, .. } if name == "br" => {
                out.push('\n');
                out.push_str(&plain_keep(kids));
            }
            Node::El { kids, .. } => out.push_str(&plain_keep(kids)),
        }
    }
    out
}

fn code_block(code: &str, lang: Option<&str>, out: &mut Out) {
    out.blank();
    out.lines.push(format!("```{}", lang.unwrap_or("")));
    out.lines.extend(code.trim_matches('\n').lines().map(str::to_string));
    out.lines.push("```".into());
    out.blank();
}

fn link(n: &Node, out: &mut Out) {
    let label = n
        .child("ac:link-body")
        .or_else(|| n.child("ac:plain-text-link-body"))
        .map(|b| plain(b.kids()))
        .filter(|t| !t.trim().is_empty());
    let target = n.kids().iter().find(|k| k.name().starts_with("ri:"));
    let text = match (label, target) {
        (Some(l), _) => l.trim().to_string(),
        (None, Some(t)) if t.name() == "ri:user" => "@someone".into(),
        (None, Some(t)) => t
            .attr("ri:content-title")
            .or_else(|| t.attr("ri:filename"))
            .or_else(|| t.attr("ri:space-key"))
            .unwrap_or("")
            .to_string(),
        (None, None) => String::new(),
    };
    if !text.is_empty() {
        out.word(&text);
    }
}

fn macro_(n: &Node, out: &mut Out) {
    let kind = n.attr("ac:name").unwrap_or("");
    let body = n.child("ac:rich-text-body");
    match kind {
        "jira" => {
            if let Some(key) = n.param("key").filter(|k| !k.is_empty()) {
                out.word(&key);
            } else if let Some(jql) = n.param("jqlQuery").filter(|k| !k.is_empty()) {
                out.word(&format!("(Jira query: {jql})"));
            }
        }
        "status" => {
            if let Some(t) = n.param("title").filter(|t| !t.is_empty()) {
                out.word(&format!("[{t}]"));
            }
        }
        "code" | "noformat" => {
            let code = n.child("ac:plain-text-body").map(|b| plain_keep(b.kids())).unwrap_or_default();
            code_block(&code, n.param("language").as_deref(), out);
        }
        "info" | "note" | "warning" | "tip" | "panel" | "expand" | "details" => {
            out.blank();
            let title = n.param("title").filter(|t| !t.is_empty());
            let label = match (kind, title) {
                (_, Some(t)) => format!("{t}:"),
                ("expand" | "details" | "panel", None) => String::new(),
                (k, None) => format!("{}{}:", k[..1].to_ascii_uppercase(), &k[1..]),
            };
            if !label.is_empty() {
                out.lines.push(format!("**{label}**"));
            }
            if let Some(b) = body {
                kids_of(b, out);
            }
            out.blank();
        }
        // Navigation and decoration: nothing to read.
        "toc" | "children" | "pagetree" | "anchor" | "recently-updated" | "contentbylabel" | "attachments"
        | "profile" | "space-attachments" => {}
        _ => {
            if let Some(b) = body {
                kids_of(b, out);
            } else if let Some(b) = n.child("ac:plain-text-body") {
                out.text(&plain(b.kids()));
            }
        }
    }
}

fn list(n: &Node, out: &mut Out) {
    out.end_line();
    let ordered = n.name() == "ol";
    let mut i = 0;
    for item in n.kids().iter().filter(|k| matches!(k.name(), "li" | "ac:task")) {
        i += 1;
        let (marker, kids): (String, &[Node]) = if item.name() == "ac:task" {
            let done = item.child("ac:task-status").map(|s| plain(s.kids())).is_some_and(|s| s.trim() == "complete");
            let body = item.child("ac:task-body").map(Node::kids).unwrap_or(&[]);
            ((if done { "- [x] " } else { "- [ ] " }).to_string(), body)
        } else if ordered {
            (format!("{i}. "), item.kids())
        } else {
            ("- ".to_string(), item.kids())
        };
        let body = render(kids);
        let pad = " ".repeat(marker.len());
        let mut first = true;
        for l in body.into_iter().filter(|l| !l.is_empty()) {
            out.lines.push(if first { format!("{marker}{l}") } else { format!("{pad}{l}") });
            first = false;
        }
        if first {
            out.lines.push(marker.trim_end().to_string());
        }
    }
    out.blank();
}

struct Cell {
    header: bool,
    lines: Vec<String>,
}

fn rows_of(table: &Node) -> Vec<Vec<Cell>> {
    let mut rows = Vec::new();
    let mut walk = |n: &Node| {
        for part in n.kids() {
            match part.name() {
                "tr" => rows.push(
                    part.kids()
                        .iter()
                        .filter(|c| matches!(c.name(), "td" | "th"))
                        .map(|c| Cell { header: c.name() == "th", lines: render(c.kids()) })
                        .collect::<Vec<_>>(),
                ),
                "thead" | "tbody" | "tfoot" => {
                    for tr in part.kids().iter().filter(|r| r.name() == "tr") {
                        rows.push(
                            tr.kids()
                                .iter()
                                .filter(|c| matches!(c.name(), "td" | "th"))
                                .map(|c| Cell { header: c.name() == "th", lines: render(c.kids()) })
                                .collect(),
                        );
                    }
                }
                _ => {}
            }
        }
    };
    walk(table);
    rows.retain(|r: &Vec<Cell>| !r.is_empty());
    rows
}

fn one_line(c: &Cell) -> String {
    c.lines.iter().filter(|l| !l.is_empty()).map(|l| l.trim()).collect::<Vec<_>>().join(" ")
}

/// Short single-line cells stay a Markdown table; anything bigger becomes one block per row.
fn table(n: &Node, out: &mut Out) {
    let rows = rows_of(n);
    out.blank();
    if rows.is_empty() {
        return;
    }
    let small = rows.iter().flatten().all(|c| c.lines.iter().filter(|l| !l.is_empty()).count() <= 1)
        && rows.iter().flatten().all(|c| one_line(c).chars().count() <= 120);
    let header_row = rows[0].len() > 1 && rows[0].iter().all(|c| c.header);
    if small {
        let width = rows.iter().map(Vec::len).max().unwrap_or(1);
        let row = |r: &[Cell]| {
            let mut cells: Vec<String> = r.iter().map(|c| one_line(c).replace('|', "\\|")).collect();
            cells.resize(width, String::new());
            format!("| {} |", cells.join(" | "))
        };
        out.lines.push(row(&rows[0]));
        out.lines.push(format!("|{}", " --- |".repeat(width)));
        for r in &rows[1..] {
            out.lines.push(row(r));
        }
        out.blank();
        return;
    }
    let headers: Vec<String> = if header_row { rows[0].iter().map(one_line).collect() } else { Vec::new() };
    let body = if header_row { &rows[1..] } else { &rows[..] };
    for (i, r) in body.iter().enumerate() {
        out.lines.push(format!("**[Row {}]**", i + 1));
        for (j, c) in r.iter().enumerate() {
            let label = match headers.get(j).filter(|h| !h.is_empty()) {
                Some(h) => h.trim_matches('*').to_string(),
                // No header row: a header cell at the start of a row labels the rest.
                None if c.header => {
                    let t = one_line(c);
                    out.lines.push(format!("{}:", t.trim_matches('*')));
                    continue;
                }
                // An unlabeled first column is nearly always the row number.
                None if j == 0 && header_row => "#".to_string(),
                None => format!("Column {}", j + 1),
            };
            let content: Vec<&String> = c.lines.iter().filter(|l| !l.is_empty()).collect();
            match content.as_slice() {
                [] => {}
                [only] if only.chars().count() <= 160 => out.lines.push(format!("{label}: {}", only.trim())),
                _ => {
                    out.lines.push(format!("{label}:"));
                    for l in &c.lines {
                        out.lines.push(if l.is_empty() { String::new() } else { format!("  {l}") });
                    }
                }
            }
        }
        out.blank();
    }
}

/// A page's storage-format body as readable text.
pub fn to_text(storage: &str) -> String {
    let lines = render(&parse(storage));
    let mut out = String::new();
    let mut blank = false;
    for l in lines {
        if l.trim().is_empty() {
            if !blank && !out.is_empty() {
                out.push('\n');
            }
            blank = true;
        } else {
            out.push_str(l.trim_end());
            out.push('\n');
            blank = false;
        }
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_requirements_table_keeps_its_lists_and_jira_keys() {
        let page = r#"<h2>Requirements</h2><table><tbody>
            <tr><th><p><strong>#</strong></p></th><th><p><strong>Requirement</strong></p></th><th><p><strong>Notes</strong></p></th></tr>
            <tr><td><p>1</p></td><td><p>Entrypoint</p></td><td>
              <p><strong>AC1. Access</strong></p>
              <p><ac:structured-macro ac:name="jira"><ac:parameter ac:name="key">LIMO-10902</ac:parameter><ac:parameter ac:name="server">System Jira</ac:parameter></ac:structured-macro></p>
              <ac:image><ri:attachment ri:filename="x.png" /></ac:image>
              <ol><li><p>Applies to all owners&nbsp;&amp; admins.</p></li>
                  <li><p>Apps</p><ol><li><p>Show the icon</p></li><li><p>Label &ldquo;Baru&rdquo;</p></li></ol></li></ol>
            </td></tr></tbody></table>"#;
        let t = to_text(page);
        assert!(t.contains("## Requirements"), "{t}");
        assert!(t.contains("**[Row 1]**\n#: 1\nRequirement: Entrypoint\nNotes:\n"), "{t}");
        assert!(t.contains("  **AC1. Access**"), "{t}");
        assert!(t.contains("  LIMO-10902"), "{t}");
        assert!(!t.contains("System Jira"), "{t}");
        assert!(t.contains("  1. Applies to all owners & admins."), "{t}");
        assert!(t.contains("  2. Apps\n     1. Show the icon\n     2. Label \"Baru\""), "{t}");
        assert!(t.contains("[image]"), "{t}");
    }

    #[test]
    fn small_tables_stay_tables_and_side_headers_label_rows() {
        let t = to_text("<table><tr><th>Metric</th><th>Target</th></tr><tr><td>MAU</td><td>10%</td></tr></table>");
        assert_eq!(t, "| Metric | Target |\n| --- | --- |\n| MAU | 10% |");
        let page = r#"<table><tr><th><p><strong>Status</strong></p></th><td><p><ac:structured-macro ac:name="status"><ac:parameter ac:name="title">DRAFT</ac:parameter></ac:structured-macro></p></td></tr>
            <tr><th><p>Owner</p></th><td><p><ac:link><ri:user ri:account-id="1" /></ac:link></p></td></tr></table>"#;
        assert_eq!(to_text(page), "| **Status** | [DRAFT] |\n| --- | --- |\n| Owner | @someone |");
    }

    #[test]
    fn nested_tables_stay_inside_their_cell() {
        let long = "word ".repeat(40);
        let page = format!(
            "<table><tr><th>Req</th><th>Notes</th></tr><tr><td>3</td><td><p>{long}</p>\
             <table><tr><th>Field</th><th>Description</th></tr><tr><td>id</td><td>Group id</td></tr></table></td></tr></table><p>After</p>"
        );
        let t = to_text(&page);
        assert!(t.contains("Notes:\n  word word"), "{t}");
        assert!(t.contains("  | Field | Description |\n  | --- | --- |\n  | id | Group id |"), "{t}");
        assert!(t.ends_with("After"), "{t}");
    }

    #[test]
    fn links_code_and_panels() {
        let page = r#"<p>See <a href="https://x.test/a">the doc</a> and <a href="https://x.test/b">https://x.test/b</a>, or
            <ac:link><ri:page ri:content-title="Runbook" /></ac:link>.</p>
            <ac:structured-macro ac:name="code"><ac:parameter ac:name="language">sql</ac:parameter><ac:plain-text-body><![CDATA[select 1
from t where a < 2]]></ac:plain-text-body></ac:structured-macro>
            <ac:structured-macro ac:name="info"><ac:rich-text-body><p>Read me</p></ac:rich-text-body></ac:structured-macro>
            <ac:structured-macro ac:name="toc" />"#;
        let t = to_text(page);
        assert!(t.starts_with("See the doc (https://x.test/a) and https://x.test/b, or Runbook."), "{t}");
        assert!(t.contains("```sql\nselect 1\nfrom t where a < 2\n```"), "{t}");
        assert!(t.contains("**Info:**\nRead me"), "{t}");
    }

    #[test]
    fn broken_markup_never_panics() {
        for bad in [
            "",
            "<",
            "<p>unclosed",
            "</p>stray",
            "<p a=>x</p>",
            "a &bogus; b &#xZZ; &",
            "<![CDATA[x",
            "<!-- x",
            "<a href='x",
        ] {
            let _ = to_text(bad);
        }
        assert_eq!(to_text("a &bogus; b &#65; &"), "a &bogus; b A &");
        assert_eq!(to_text("<p>one<br/>two</p>"), "one\ntwo");
    }
}
