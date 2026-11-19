//! Takes out of a text anything shaped like a credential. The Proxy keeps moor's own tokens
//! out of the Box, but a command can still print one it found, so every text kept is scrubbed.

use std::sync::LazyLock;

use regex::Regex;

/// What stands in for a credential.
const GONE: &str = "[redacted]";

/// Each pattern and its replacement, in order: a private key, a URL's password, a known token
/// shape, an auth header, then a value assigned to a name that says it is secret.
const RULES: [(&str, &str); 5] = [
    (
        r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY-----|\z)",
        "[redacted private key]",
    ),
    (
        r"([a-zA-Z][a-zA-Z0-9+.-]*://)[^/\s:@]+:[^/\s@]+@",
        "${1}[redacted]@",
    ),
    (
        r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-ant-[A-Za-z0-9_-]{10,}|sk-[A-Za-z0-9_-]{20,}|xox[abposr]-[A-Za-z0-9-]{10,}|(?:AKIA|ASIA)[0-9A-Z]{16}|glpat-[A-Za-z0-9_-]{20,}|npm_[A-Za-z0-9]{30,}|AIza[0-9A-Za-z_-]{35}|eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,})",
        GONE,
    ),
    (
        r"(?i)\b(bearer|basic|token)(\s+)[A-Za-z0-9._~+/=-]{16,}",
        "${1}${2}[redacted]",
    ),
    (
        r#"(?i)\b([A-Z0-9_.-]*(?:token|secret|password|passwd|api[_-]?key|private[_-]?key|credentials?|access[_-]?key)["']?\s*[:=]\s*)(?:"[^"\n]+"|'[^'\n]+'|[A-Za-z0-9_./+=-]{8,}(\s|$|[,;]))"#,
        "${1}[redacted]${2}",
    ),
];

/// The rules, compiled once; a test keeps every one compiling.
static COMPILED: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    RULES
        .iter()
        .filter_map(|(p, with)| Regex::new(p).ok().map(|re| (re, *with)))
        .collect()
});

/// `text` with every credential-shaped part replaced.
#[must_use]
pub fn scrub(text: &str) -> String {
    COMPILED.iter().fold(text.to_owned(), |t, (re, with)| {
        re.replace_all(&t, *with).into_owned()
    })
}

/// How many rules compiled.
#[cfg(test)]
pub(super) fn compiled() -> usize {
    COMPILED.len()
}
