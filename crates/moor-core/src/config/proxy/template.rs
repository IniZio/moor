//! Placeholder checking and filling for rule templates.

/// Placeholders a rule's `inject.form` may use.
pub(super) const FORM_SLOTS: &[&str] = &["secret"];
/// Placeholders `allow` paths and `box_env` values may use.
pub(super) const REPO_SLOTS: &[&str] = &["repo", "repo:urlenc"];
/// Placeholders `allow` paths may use: the repo slots plus one repo the Box may only read.
pub(super) const ALLOW_SLOTS: &[&str] = &["repo", "repo:urlenc", "read_repo"];
/// The slot an allow entry repeats once per read-only repo.
pub(super) const READ_REPO: &str = "{read_repo}";

/// Whether `s` names the `{repo}` or `{repo:urlenc}` slot.
pub(super) fn uses_repo(s: &str) -> bool {
    s.contains("{repo}") || s.contains("{repo:urlenc}")
}

/// Every `{slot}` in `s` is one of `allowed` and braces balance.
pub(super) fn check_slots(s: &str, allowed: &[&str]) -> Result<(), String> {
    let mut parts = s.split('{');
    if parts.next().unwrap_or_default().contains('}') {
        return Err(format!("unbalanced braces in {s:?}"));
    }
    for part in parts {
        let Some((slot, after)) = part.split_once('}') else {
            return Err(format!("unclosed placeholder in {s:?}"));
        };
        if !allowed.contains(&slot) {
            let known: Vec<String> = allowed.iter().map(|a| format!("{{{a}}}")).collect();
            return Err(format!(
                "unknown placeholder {{{slot}}}; known: {}",
                known.join(", ")
            ));
        }
        if after.contains('}') {
            return Err(format!("unbalanced braces in {s:?}"));
        }
    }
    Ok(())
}

/// Replaces `{repo}` with `owner/name` and `{repo:urlenc}` with its percent-encoding.
pub(super) fn fill_repo(s: &str, owner: &str, name: &str) -> String {
    let plain = format!("{owner}/{name}");
    s.replace("{repo:urlenc}", &percent_encode(&plain))
        .replace("{repo}", &plain)
}

/// Replaces `{read_repo}` with `owner/name`.
pub(super) fn fill_read_repo(s: &str, owner: &str, name: &str) -> String {
    s.replace(READ_REPO, &format!("{owner}/{name}"))
}

/// Percent-encodes every byte outside the RFC 3986 unreserved set.
fn percent_encode(s: &str) -> String {
    use std::fmt::Write as _;
    s.bytes().fold(String::new(), |mut out, b| {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
        out
    })
}
