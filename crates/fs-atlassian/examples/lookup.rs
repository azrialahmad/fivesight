//! Try the read-only lookups against your own Atlassian site, exactly as Ask runs them.
//!
//! cargo run -p fs-atlassian --example lookup -- status
//! cargo run -p fs-atlassian --example lookup -- search "assignee = currentUser() ORDER BY updated DESC"
//! cargo run -p fs-atlassian --example lookup -- view PAY-142
//! cargo run -p fs-atlassian --example lookup -- csearch "retry policy"
//! cargo run -p fs-atlassian --example lookup -- page 123456

use fs_atlassian::{run, status, ReadCommand};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = match args.first().map(String::as_str) {
        Some("status") => {
            println!("{:#?}", status());
            return;
        }
        Some("search") => ReadCommand::JiraSearch { jql: args.get(1).cloned().unwrap_or_default(), limit: 5 },
        Some("view") => ReadCommand::JiraView { key: args.get(1).cloned().unwrap_or_default() },
        Some("csearch") => ReadCommand::ConfluenceSearch { query: args.get(1).cloned().unwrap_or_default(), limit: 5 },
        Some("find") => {
            ReadCommand::ConfluenceFind { title: args.get(1).cloned().unwrap_or_default(), space: args.get(2).cloned() }
        }
        Some("page") => ReadCommand::ConfluencePage { page_id: args.get(1).cloned().unwrap_or_default() },
        _ => {
            eprintln!(
                "usage: lookup status | search <jql> | view <KEY> | csearch <words> | find <title> [space] | page <id>"
            );
            std::process::exit(2);
        }
    };
    println!("{}", cmd.label());
    match run(&cmd) {
        Ok(text) => println!("{text}"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
