//! Puts the pinned `gh` into the Box: `gh pr create --attach` needs gh 2.99 or newer.

use moor_core::seam::engine::LaunchNotice;

use super::super::nono_release::GH_VERSION;

/// Where `gh` lives, relative to the Box home.
pub(super) const GH_PATH: &str = ".local/bin/gh";

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
        let home = std::env::temp_dir().join(format!("moor-gh-check-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        if let Some(body) = body {
            std::fs::write(home.join(GH_PATH), body).unwrap();
        }
        let out = Command::new("sh")
            .args(["-c", &check_script(sha256)])
            .env("HOME", &home)
            .output()
            .unwrap();
        std::fs::remove_dir_all(&home).unwrap();
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
