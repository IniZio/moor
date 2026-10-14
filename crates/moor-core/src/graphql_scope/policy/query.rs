//! Allow-list for the text of a `search` query: only the Task repo and the qualifiers gh emits.

use super::super::{Reason, RepoRef};

/// Qualifiers that take one of a fixed set of values.
const FIXED: &[(&str, &[&str])] = &[
    ("is", &["pr", "issue", "open", "closed", "merged", "draft"]),
    ("state", &["open", "closed", "merged"]),
    ("type", &["pr", "issue"]),
    ("draft", &["true", "false"]),
    ("archived", &["false"]),
    (
        "review",
        &["none", "required", "approved", "changes_requested"],
    ),
    ("status", &["success", "failure", "pending"]),
];

/// Qualifiers that take a login, ref, label or milestone name.
const NAMED: &[&str] = &[
    "author",
    "assignee",
    "review-requested",
    "reviewed-by",
    "involves",
    "mentions",
    "head",
    "base",
    "label",
    "milestone",
];

/// Returns whether a character may appear in a login, ref or label.
const fn name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '@' | '-')
}

/// Returns whether a value is a non-empty name.
fn is_name(value: &str) -> bool {
    !value.is_empty() && value.chars().all(name_char)
}

/// Returns whether a value is `<field>-asc` or `<field>-desc`.
fn is_sort(value: &str) -> bool {
    value
        .strip_suffix("-asc")
        .or_else(|| value.strip_suffix("-desc"))
        .is_some_and(|field| {
            !field.is_empty() && field.chars().all(|c| c.is_ascii_lowercase() || c == '-')
        })
}

/// Returns whether a qualifier and value are allowed, other than `repo`.
fn qualifier_ok(key: &str, value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if let Some((_, values)) = FIXED.iter().find(|(k, _)| *k == key) {
        return values.contains(&lower.as_str());
    }
    if key == "sort" {
        return is_sort(&lower);
    }
    NAMED.contains(&key) && is_name(value)
}

/// Requires every token to be allowed and the query to name the Task repo exactly once.
pub(super) fn search_scope(query: &str, repo: &RepoRef) -> Result<(), Reason> {
    let want = format!("{}/{}", repo.owner, repo.name);
    let mut seen = false;
    for token in query.split_ascii_whitespace() {
        let denied = || Reason::new(format!("search term `{token}` is not allowed"));
        let Some((key, value)) = token.split_once(':') else {
            let word = token.chars().all(|c| name_char(c) && c != '/' && c != '@');
            if !word || matches!(token.to_ascii_lowercase().as_str(), "or" | "and" | "not") {
                return Err(denied());
            }
            continue;
        };
        let key = key.to_ascii_lowercase();
        if key == "repo" {
            if seen || !value.eq_ignore_ascii_case(&want) {
                return Err(denied());
            }
            seen = true;
        } else if !qualifier_ok(&key, value) {
            return Err(denied());
        }
    }
    if seen {
        Ok(())
    } else {
        Err(Reason::new("search query needs `repo:<owner>/<name>`"))
    }
}
