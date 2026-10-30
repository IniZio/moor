//! Script that checks a person's agent profile out at one commit inside the Box.

use super::quote;

/// Where the profile is checked out, relative to the Box user's home.
pub const PROFILE_DIR: &str = ".moor-profile/person";

/// Under the Box workdir: one line per profile step that failed at launch, `<what> exit <code>`,
/// rewritten on every launch that applies a profile.
pub const FAILURES_FILE: &str = ".moor/profile-failures";

/// Shell text that leaves `$HOME/.moor-profile/person` at exactly `sha` of `url`.
///
/// A fresh `git init` plus a shallow fetch of the sha works for any commit, branch head or
/// not; an earlier checkout is replaced so a rerun converges.
#[must_use]
pub fn fetch_script(url: &str, sha: &str) -> String {
    format!(
        "set -e\nd=\"$HOME/{PROFILE_DIR}\"\nrm -rf \"$d\"\nmkdir -p \"$d\"\ncd \"$d\"\n\
         git init -q\ngit remote add origin {url}\ngit fetch -q --depth 1 origin {sha}\n\
         git -c advice.detachedHead=false checkout -q FETCH_HEAD\n",
        url = quote(url),
        sha = quote(sha),
    )
}
