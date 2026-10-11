//! Clock ban: only the clock adapter may read the wall clock.

use crate::starts_with;

/// Constructors that read the wall clock; only the clock adapter may call them.
pub(crate) const BANNED_CALLS: [(&str, &str); 2] = [("Instant", "now"), ("SystemTime", "now")];
/// Crates the clock ban always covers, with or without a layering rule.
pub(crate) const BAN_CRATES: [&str; 2] = ["moor", "moor_core"];
/// Module (in crate `moor`) allowed to call the banned constructors.
pub(crate) const BAN_ALLOWED: [&str; 2] = ["adapters", "clock"];
pub(crate) const BAN_CRATE_ALLOWED: &str = "moor";

/// One call to a banned constructor, e.g. `Instant::now`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// Source line of the first path segment.
    pub line: usize,
    /// Module containing the call.
    pub from: Vec<String>,
    /// Banned call as written without qualification, e.g. `Instant::now`.
    pub name: String,
}

/// Banned calls outside the one module allowed to make them.
#[must_use]
pub fn clock_violations<'a>(all: &'a [Call], krate: &str) -> Vec<&'a Call> {
    let allowed: Vec<String> = BAN_ALLOWED.iter().map(|s| (*s).to_owned()).collect();
    all.iter()
        .filter(|c| krate != BAN_CRATE_ALLOWED || !starts_with(&c.from, &allowed))
        .collect()
}
