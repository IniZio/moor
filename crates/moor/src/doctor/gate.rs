//! The gate check, made only when the gate's public URL is known.

use super::{Check, Inputs};

/// Where a running gate publishes its resource metadata.
const METADATA_PATH: &str = "/.well-known/oauth-protected-resource";

/// Whether the gate answers at its public URL; none when no URL is known.
pub(super) fn check(i: &Inputs<'_>) -> Option<Check> {
    let base = i.public_url?.trim_end_matches('/');
    let url = format!("{base}{METADATA_PATH}");
    let fix = "run moor-gate to let teammates connect";
    Some(match (i.get)(&url) {
        Ok(200) => Check::ok("gate", format!("{base} answers")),
        Ok(code) => Check::warn("gate", format!("{url} answered {code}"), fix),
        Err(why) => Check::warn("gate", format!("{url} is unreachable: {why}"), fix),
    })
}
