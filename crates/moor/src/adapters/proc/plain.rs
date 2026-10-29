//! The last thing a failed tool printed, cut down to words that cannot carry a secret.
//!
//! A word passes only when it is plain prose, a config field path or a short number; anything
//! else (tokens, keys, addresses, ids, URLs with queries) becomes `…`.

/// Longest sentence kept, in characters.
const MAX: usize = 300;
/// Longest word kept, in characters.
const WORD_MAX: usize = 48;

/// The last non-empty line of `stderr` in plain words, or `None` when nothing plain is left.
pub(crate) fn last_sentence(stderr: &str) -> Option<String> {
    let line = stderr.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    let mut words: Vec<&str> = Vec::new();
    for w in line.split_whitespace() {
        let kept = if plain(w) { w } else { "…" };
        if !(kept == "…" && words.last() == Some(&"…")) {
            words.push(kept);
        }
    }
    if !words.iter().any(|w| *w != "…") {
        return None;
    }
    let joined = words.join(" ");
    Some(if joined.chars().count() > MAX {
        let cut: String = joined.chars().take(MAX - 1).collect();
        format!("{cut}…")
    } else {
        joined
    })
}

/// Whether `w` is a word that cannot hold a secret.
fn plain(w: &str) -> bool {
    if w.chars().count() > WORD_MAX {
        return false;
    }
    let punct = |c: char| ".,;:()'\"=_-/".contains(c);
    let number = w.chars().all(|c| c.is_ascii_digit() || punct(c)) && w.len() <= 8;
    let prose = w.chars().all(|c| c.is_ascii_alphabetic() || punct(c))
        && w.chars().filter(char::is_ascii_uppercase).count() <= 3;
    number || prose
}

#[cfg(test)]
mod tests;
