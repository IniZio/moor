//! Allow-list for the text of a `search` query: only the Box repo and the qualifiers gh emits.

use super::super::{shown, Reason, RepoRef};

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

/// Deepest parenthesis nesting a query may use.
const MAX_DEPTH: usize = 4;

/// The whitespace-separated words of a query, with each leading `(` and trailing `)` of a
/// word split off as a token of its own.
fn tokens(query: &str) -> impl Iterator<Item = &str> {
    query.split_ascii_whitespace().flat_map(|word| {
        let inner = word.trim_start_matches('(');
        let core = inner.trim_end_matches(')');
        let opens = std::iter::repeat_n("(", word.len() - inner.len());
        let closes = std::iter::repeat_n(")", inner.len() - core.len());
        opens
            .chain(Some(core).filter(|c| !c.is_empty()))
            .chain(closes)
    })
}

/// Requires every token to be allowed and the query to name the Box repo exactly once,
/// outside any group. With `grouped` the query may hold parenthesised groups, with `OR`,
/// `AND` and `NOT` only inside them, as `gh pr list --search` sends: the Box repo term is
/// joined by AND to every group, so no group widens the search past it. Only advanced search
/// reads groups that way. A refusal carries the query with its values hidden.
pub(super) fn search_scope(query: &str, repo: &RepoRef, grouped: bool) -> Result<(), Reason> {
    let want = format!("{}/{}", repo.owner, repo.name);
    scope(query, &want, grouped).map_err(|reason| reason.with_query(masked(query, &want)))
}

/// Longest masked query kept, in characters.
const MAX_MASKED: usize = 200;
/// Longest qualifier name a masked query shows.
const MAX_KEY: usize = 24;

/// `query` with grouping, operators, qualifier names, fixed values and the Box repo kept,
/// and every other value and word shown as `…`.
fn masked(query: &str, want: &str) -> String {
    let words: Vec<String> = tokens(query).map(|t| mask(t, want)).collect();
    words.join(" ").chars().take(MAX_MASKED).collect()
}

/// One token of [`masked`].
fn mask(token: &str, want: &str) -> String {
    let lower = token.to_ascii_lowercase();
    if matches!(lower.as_str(), "(" | ")" | "or" | "and" | "not") {
        return token.to_owned();
    }
    let Some((key, value)) = token.split_once(':') else {
        return "…".to_owned();
    };
    let plain_key = key.len() <= MAX_KEY
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'+' | b'~' | b'_'));
    let key = if plain_key { key } else { "…" };
    let lower_key = key.to_ascii_lowercase();
    let fixed = FIXED.iter().any(|(k, values)| {
        *k == lower_key && values.contains(&value.to_ascii_lowercase().as_str())
    });
    let value = if lower_key == "repo" && value.eq_ignore_ascii_case(want) {
        "{repo}"
    } else if fixed || value.is_empty() {
        value
    } else {
        "…"
    };
    format!("{key}:{value}")
}

/// The check of [`search_scope`].
fn scope(query: &str, want: &str, grouped: bool) -> Result<(), Reason> {
    let mut seen = false;
    let mut depth = 0_usize;
    for token in tokens(query) {
        let denied = || Reason::new(format!("search term `{}` is not allowed", shown(token)));
        match token {
            "(" if grouped && depth < MAX_DEPTH => depth += 1,
            ")" if grouped && depth > 0 => depth -= 1,
            _ => match token.split_once(':') {
                None => {
                    let word = token.chars().all(|c| name_char(c) && c != '/' && c != '@');
                    let operator =
                        matches!(token.to_ascii_lowercase().as_str(), "or" | "and" | "not");
                    if !word || (operator && depth == 0) {
                        return Err(denied());
                    }
                }
                Some((key, value)) if key.eq_ignore_ascii_case("repo") => {
                    if seen || depth > 0 || !value.eq_ignore_ascii_case(want) {
                        return Err(denied());
                    }
                    seen = true;
                }
                Some((key, value)) => {
                    if !qualifier_ok(&key.to_ascii_lowercase(), value) {
                        return Err(denied());
                    }
                }
            },
        }
    }
    if depth > 0 {
        Err(Reason::new("search query has an unclosed `(`"))
    } else if seen {
        Ok(())
    } else {
        Err(Reason::new("search query needs `repo:<owner>/<name>`"))
    }
}
