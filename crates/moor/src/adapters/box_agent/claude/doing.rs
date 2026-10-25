//! The `doing` line of a tool-use record: the tool's name, plus a short argument only for
//! tools whose argument reduces to a word that cannot hold a secret or file contents.
//!
//! A shell command keeps its program name and a plain lowercase subcommand (`cargo test`),
//! never flags, values or later words, where tokens and paths live. A file tool keeps the
//! file's base name. A fetch or browser tool keeps the host, never the path, query or user
//! part of the URL. Every other tool keeps its name alone: its input (search patterns,
//! prompts, issue bodies) has no part that is safe by shape.

use moor_core::doing::doing_line;
use serde_json::Value;

/// Tools whose input names one file.
const FILE_TOOLS: [&str; 5] = ["Read", "Edit", "Write", "MultiEdit", "NotebookEdit"];

/// The line for a record's `tool` and `tool_input`; none when the tool name is missing or odd.
pub(super) fn line(record: &Value) -> Option<String> {
    let tool = record.get("tool").and_then(Value::as_str)?;
    let plain = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    if tool.is_empty() || tool.len() > 64 || !tool.chars().all(plain) {
        return None;
    }
    let input = record.get("tool_input");
    let field = |key: &str| input.and_then(|i| i.get(key)).and_then(Value::as_str);
    let arg = if tool == "Bash" {
        field("command").and_then(program)
    } else if FILE_TOOLS.contains(&tool) {
        field("file_path")
            .or_else(|| field("notebook_path"))
            .and_then(base_name)
    } else if tool == "WebFetch" || tool.contains("browser") {
        field("url").and_then(host)
    } else {
        None
    };
    doing_line(&arg.map_or_else(|| tool.to_owned(), |a| format!("{tool}: {a}")))
}

/// The program a command runs, past `NAME=value` words and a leading `cd <dir> &&` or `;`, as
/// a base name, plus its subcommand when that is a plain lowercase word.
fn program(command: &str) -> Option<String> {
    let after_cd = command.trim_start().strip_prefix("cd ").and_then(|rest| {
        rest.split_once("&&")
            .or_else(|| rest.split_once(';'))
            .map(|(_, after)| after)
    });
    if let Some(after) = after_cd {
        return program(after);
    }
    let mut words = command
        .split_whitespace()
        .skip_while(|w| w.contains('=') && !w.starts_with('='));
    let name = base_name(words.next()?)?;
    let safe = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+');
    if !name.chars().all(safe) {
        return None;
    }
    match words.next() {
        Some(sub) if sub.len() <= 20 && sub.chars().all(|c| c.is_ascii_lowercase()) => {
            Some(format!("{name} {sub}"))
        }
        _ => Some(name),
    }
}

/// The last part of a path; none when it is empty or holds a control character.
fn base_name(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?.trim();
    (!name.is_empty() && !name.chars().any(char::is_control)).then(|| name.to_owned())
}

/// The host and port of a URL, without scheme, user part, path, query or fragment.
fn host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']');
    (!host.is_empty() && host.chars().all(ok)).then(|| host.to_owned())
}
