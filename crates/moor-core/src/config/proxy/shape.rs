//! Turns a concrete request into a templated `allow` entry.

/// Shortest hex segment treated as a git object id (abbreviated oids start at 7).
const MIN_OID_LEN: usize = 7;

/// Slot for the Box repo as `owner/name`.
const REPO_SLOT: &str = "{repo}";

/// Path words followed by a name that is free text: it becomes `*`.
const NAME_SLOTS: &[&str] = &[
    "workflows",
    "labels",
    "environments",
    "secrets",
    "variables",
];

/// Path words followed by free text (names, file paths): the rest becomes `**`.
const FREE_TAILS: &[&str] = &[
    "contents", "branches", "compare", "tags", "trees", "blobs", "readme",
];

/// Path words followed by `heads` or `tags` (kept) and then a free-text name.
const REF_TAILS: &[&str] = &["refs", "ref", "matching-refs"];

/// Templates a request as an `allow` entry: `METHOD /path`.
///
/// The path is normalised as the proxy matcher does (percent-decoded, empty
/// and `.` segments dropped, `..` resolved). The Box repo's `owner/name`
/// after `repos` becomes `{repo}`; another repo's owner and name stay literal.
/// Numeric and hex object-id segments become `*`, as does the name after a
/// name slot such as `workflows`. Text after a free-text word such as
/// `contents` becomes `**`; refs keep `heads` or `tags`. Every other segment
/// stays literal.
///
/// Returns `None` when no sound entry exists: the method is not upper-case
/// letters, a segment outside a free-text position has characters beyond
/// `[A-Za-z0-9._-]`, or the path is one the proxy refuses as ambiguous
/// (`;`, `\`, control characters, `%2e`/`%2f`, a `%` left after one decode).
/// The query and fragment are dropped.
#[must_use]
pub fn request_shape(method: &str, target: &str, repo: (&str, &str)) -> Option<String> {
    if method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }
    let target = target.split('#').next().unwrap_or_default();
    let path = target.split('?').next().unwrap_or_default();
    let decoded = decode_path(path)?;

    let segments = normalise(&decoded);
    let mut out = String::new();
    let mut i = 0;
    while let Some(&seg) = segments.get(i) {
        i += 1;
        out.push('/');
        if is_id(seg) {
            out.push('*');
            continue;
        }
        out.push_str(plain(seg)?);
        let (used, done) = push_after(&mut out, seg, segments.get(i..).unwrap_or_default(), repo)?;
        i += used;
        if done {
            break;
        }
    }
    if out.is_empty() {
        out.push('/');
    }
    Some(format!("{method} {out}"))
}

/// Appends what follows the literal `seg`, per its kind. Returns the segments
/// used and whether the path ends here.
fn push_after(
    out: &mut String,
    seg: &str,
    rest: &[&str],
    repo: (&str, &str),
) -> Option<(usize, bool)> {
    if seg == "repos" {
        return Some((push_repo(out, rest, repo)?, false));
    }
    if NAME_SLOTS.contains(&seg) {
        if rest.first().is_none_or(|&next| next == "public-key") {
            return Some((0, false));
        }
        out.push_str("/*");
        return Some((1, false));
    }
    if REF_TAILS.contains(&seg) {
        push_ref_tail(out, rest);
        return Some((0, true));
    }
    if FREE_TAILS.contains(&seg) {
        if !rest.is_empty() {
            out.push_str("/**");
        }
        return Some((0, true));
    }
    Some((0, false))
}

/// Splits a decoded path into segments, dropping empty and `.` and resolving `..`.
fn normalise(decoded: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    for seg in decoded.split('/').filter(|s| !s.is_empty()) {
        match seg {
            "." => {}
            ".." => {
                segments.pop();
            }
            _ => segments.push(seg),
        }
    }
    segments
}

/// Appends what follows `refs`: the ref kind (`heads`, `tags`, `notes`, ...)
/// kept, then `**` for the name. A lone segment is a name unless it is `heads`
/// or `tags`.
fn push_ref_tail(out: &mut String, rest: &[&str]) {
    let Some(&kind) = rest.first() else {
        return;
    };
    let known = kind == "heads" || kind == "tags";
    match plain(kind) {
        Some(kind) if !is_id(kind) && (known || rest.len() > 1) => {
            out.push('/');
            out.push_str(kind);
            if rest.len() > 1 {
                out.push_str("/**");
            }
        }
        _ => out.push_str("/**"),
    }
}

/// Appends the owner and name following `repos` (`{repo}` for the Box repo)
/// and returns how many segments it used.
fn push_repo(out: &mut String, rest: &[&str], (owner, name): (&str, &str)) -> Option<usize> {
    let Some(&o) = rest.first() else {
        return Some(0);
    };
    let Some(&n) = rest.get(1) else {
        out.push('/');
        out.push_str(plain(o)?);
        return Some(1);
    };
    if o == owner && n == name {
        out.push('/');
        out.push_str(REPO_SLOT);
    } else {
        for part in [o, n] {
            out.push('/');
            out.push_str(plain(part)?);
        }
    }
    Some(2)
}

/// Whether a segment is an identifier: all digits, or a hex object id.
fn is_id(seg: &str) -> bool {
    seg.bytes().all(|b| b.is_ascii_digit())
        || (seg.len() >= MIN_OID_LEN && seg.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The segment when it has only `[A-Za-z0-9._-]`.
fn plain(seg: &str) -> Option<&str> {
    let ok = !seg.is_empty()
        && seg
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    ok.then_some(seg)
}

/// Percent-decodes `path` once; `None` when the proxy would refuse it as ambiguous.
fn decode_path(path: &str) -> Option<String> {
    let lower = path.to_ascii_lowercase();
    if lower.contains("%2e") || lower.contains("%2f") {
        return None;
    }
    if path
        .bytes()
        .any(|b| b == b';' || b == b'\\' || b.is_ascii_control())
    {
        return None;
    }
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hex = path.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    let decoded = String::from_utf8_lossy(&out).into_owned();
    if decoded.contains('%') || decoded.chars().any(char::is_control) {
        return None;
    }
    Some(decoded)
}

#[cfg(test)]
mod tests;
