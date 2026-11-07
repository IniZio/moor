//! Script that checks a person's agent profile out at one commit inside the Box.

use super::{quote, AGENT_PATH_LINE};

/// Where the profile is checked out, relative to the Box user's home.
pub const PROFILE_DIR: &str = ".moor-profile/person";

/// Under the Box workdir: one line per profile step that failed at launch, `<what> exit <code>`,
/// rewritten on every launch that applies a profile.
pub const FAILURES_FILE: &str = ".moor/profile-failures";

/// What a profile step prints before `<what> exit <code>` when it fails and goes on.
pub const FAILED_PREFIX: &str = "moor: profile: ";

/// The installs in `output` that printed [`FAILED_PREFIX`], each as `<what> exit <code>`.
#[must_use]
pub fn failed_installs(output: &str) -> Vec<&str> {
    output
        .lines()
        .filter_map(|l| l.trim_end().strip_prefix(FAILED_PREFIX))
        .collect()
}

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

/// The script at the checkout's root that sets the Box up for the profile's plugins.
pub const SETUP_FILE: &str = "setup.sh";

/// Shell text that runs the checkout's `setup.sh` from the checkout with the agent's `PATH`, and
/// does nothing when the profile has none.
#[must_use]
pub fn setup_script() -> String {
    format!(
        "{AGENT_PATH_LINE}\nd=\"$HOME/{PROFILE_DIR}\"\ncd \"$d\" || exit 1\n[ -f {SETUP_FILE} ] || exit 0\nsh {SETUP_FILE}\n"
    )
}

#[cfg(test)]
mod tests;
