//! Which handling a request to the judged host gets, decided from its path alone.

use super::repo_ids::PREFIX as ID_PREFIX;

/// Which handling a request path gets.
pub(super) enum Route {
    /// Forwarded without looking at the body.
    Passthrough,
    /// The GraphQL endpoint, however it is spelled.
    Graphql,
    /// The numeric repo-id form, however it is spelled, with its canonical path.
    RepoId(String),
}

/// Decides the route from the path alone.
pub(super) fn classify(target: &str) -> Route {
    let canonical = canonical_path(target);
    if canonical == "/graphql" {
        Route::Graphql
    } else if canonical == ID_PREFIX || canonical.starts_with(&format!("{ID_PREFIX}/")) {
        Route::RepoId(canonical)
    } else {
        Route::Passthrough
    }
}

/// The path part of `target` percent-decoded, lowercased, with backslashes
/// as slashes, dot segments resolved and empty segments dropped.
fn canonical_path(target: &str) -> String {
    let path = target.split(['?', '#']).next().unwrap_or_default();
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        if let (b'%', Some(v)) = (b, hex) {
            decoded.push(v);
            i += 3;
        } else {
            decoded.push(b);
            i += 1;
        }
    }
    let text = String::from_utf8_lossy(&decoded)
        .to_ascii_lowercase()
        .replace('\\', "/");
    let mut segments: Vec<&str> = Vec::new();
    for seg in text.split('/') {
        match seg {
            "" | "." => {}
            ".." => drop(segments.pop()),
            s => segments.push(s),
        }
    }
    format!("/{}", segments.join("/"))
}
