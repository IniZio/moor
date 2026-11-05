//! Puts the pinned `gh` into the Box: `gh pr create --attach` needs gh 2.99 or newer.

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{BoxStep, Driver, ExecOutcome, StepDone};
use moor_core::seam::engine::LaunchNotice;
use moor_core::seam::proc::Deadline;

use super::super::gh_release::{sha256_hex, GhRelease, GH_VERSION};
use super::INSTALL_DEADLINE;

/// Where `gh` lives, relative to the Box home.
pub(super) const GH_PATH: &str = ".local/bin/gh";

/// Prints the Box's home, then its operating system and architecture.
const HOME_AND_PLATFORM: &str = "printf '%s\\n' \"$HOME\"; uname -s -m";

/// The check script's exit code: `gh` is missing or is not the pinned binary.
pub(super) const NOT_INSTALLED: i32 = 10;

/// The POSIX shell script that checks the Box's `gh` is the pinned binary, with sha256 `sha256`;
/// run via `sh -c`. It needs no network.
pub(super) fn check_script(sha256: &str) -> String {
    let missing = NOT_INSTALLED;
    format!(
        r#"dest="$HOME/{GH_PATH}"
sum=$(sha256sum "$dest" 2>/dev/null || shasum -a 256 "$dest" 2>/dev/null)
[ "${{sum%% *}}" = "{sha256}" ] || exit {missing}
"#
    )
}

/// The warning for a `gh` that could not be put in the Box: the launch goes on without it.
pub(super) fn not_delivered(reason: &impl std::fmt::Display) -> LaunchNotice {
    LaunchNotice {
        code: "gh_not_delivered".to_owned(),
        message: format!(
            "gh {GH_VERSION} could not be put in the Box: {reason}; the agent runs without gh, \
             so `gh pr create --attach` is unavailable"
        ),
    }
}

/// Puts the pinned `gh` in the Box behind `handle` when it is missing; the notice says why it
/// could not. The launch keeps its own check, since a Box can lose `gh` before then.
pub(in crate::adapters::box_agent) fn deliver(
    driver: &dyn Driver,
    release: &GhRelease,
    handle: &BoxHandle,
) -> Option<LaunchNotice> {
    put_if_missing(driver, release, handle)
        .err()
        .map(|reason| not_delivered(&reason))
}

/// The steps of [`deliver`]; the error is why `gh` is not in the Box.
fn put_if_missing(
    driver: &dyn Driver,
    release: &GhRelease,
    handle: &BoxHandle,
) -> Result<(), String> {
    let (out, _) = run(driver, handle, HOME_AND_PLATFORM.to_owned())?;
    let mut lines = out.lines();
    let (home, platform) = (lines.next().unwrap_or_default(), lines.next());
    let mut words = platform.unwrap_or_default().split_whitespace();
    let (Some(os), Some(arch)) = (words.next(), words.next()) else {
        return Err("could not read the Box's home and platform".to_owned());
    };
    let bytes = release.gh_binary(os, arch).map_err(|e| e.to_string())?;
    match run(driver, handle, check_script(&sha256_hex(&bytes)))? {
        (_, 0) => Ok(()),
        (_, NOT_INSTALLED) => {
            let path = BoxPath::under(home, GH_PATH).map_err(|e| e.to_string())?;
            let put = BoxStep::Put {
                path,
                bytes,
                mode: 0o755,
            };
            step(driver, handle, put).map(drop)
        }
        (_, code) => Err(format!("the check exited {code}")),
    }
}

/// What `script` printed in the Box, and its exit code.
fn run(driver: &dyn Driver, handle: &BoxHandle, script: String) -> Result<(String, i32), String> {
    match step(driver, handle, BoxStep::Run(script))? {
        StepDone::Ran {
            outcome: ExecOutcome::Exited { code },
            stdout,
        } => Ok((stdout, code)),
        _ => Err("the step was interrupted".to_owned()),
    }
}

/// Runs one `step` in the Box.
fn step(driver: &dyn Driver, handle: &BoxHandle, step: BoxStep) -> Result<StepDone, String> {
    let done = driver.batch(handle, &[], &[step], Deadline(INSTALL_DEADLINE));
    let first = done.into_iter().next().ok_or("the step did not run")?;
    first.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::process::Command;

    const BODY: &[u8] = b"pinned gh binary";

    fn sum(body: &[u8]) -> String {
        format!("{:x}", Sha256::digest(body))
    }

    /// Exit code of the check script for `sha256`, with `gh` holding `body` under a fresh home.
    fn run(tag: &str, body: Option<&[u8]>, sha256: &str) -> Option<i32> {
        let dir = tempfile::Builder::new()
            .prefix(&format!("moor-gh-check-{tag}-"))
            .tempdir()
            .unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        if let Some(body) = body {
            std::fs::write(home.join(GH_PATH), body).unwrap();
        }
        let out = Command::new("sh")
            .args(["-c", &check_script(sha256)])
            .env("HOME", home)
            .output()
            .unwrap();
        out.status.code()
    }

    #[test]
    fn script_names_the_sha_and_path() {
        let s = check_script(&sum(BODY));
        assert!(s.contains(&sum(BODY)));
        assert!(s.contains(".local/bin/gh"));
        assert!(!s.contains("curl") && !s.contains("wget"));
    }

    #[test]
    fn script_is_valid_sh() {
        let out = Command::new("sh")
            .args(["-n", "-c", &check_script(&sum(BODY))])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn matching_binary_exits_zero() {
        assert_eq!(run("match", Some(BODY), &sum(BODY)), Some(0));
    }

    #[test]
    fn different_binary_is_not_installed() {
        assert_eq!(
            run("differs", Some(b"other"), &sum(BODY)),
            Some(NOT_INSTALLED)
        );
    }

    #[test]
    fn missing_binary_is_not_installed() {
        assert_eq!(run("missing", None, &sum(BODY)), Some(NOT_INSTALLED));
    }

    #[test]
    fn not_delivered_names_gh_reason_and_the_consequence() {
        let w = not_delivered(&"upload timed out");
        assert_eq!(w.code, "gh_not_delivered");
        assert!(w.message.contains("2.102.0"), "{}", w.message);
        assert!(w.message.contains("upload timed out"), "{}", w.message);
        assert!(w.message.contains("without gh"), "{}", w.message);
        assert!(w.message.contains("--attach"), "{}", w.message);
    }
}
