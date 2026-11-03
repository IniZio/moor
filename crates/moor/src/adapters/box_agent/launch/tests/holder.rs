//! The herdr-core install script run under a real `sh` in a temp `HOME` with a `PATH` of fakes.

use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;

use sha2::{Digest, Sha256};

use super::*;

/// A `curl` that logs its call and leaves `$HOME/release.tar.gz` in its `-o` file.
const FAKE_CURL: &str = "#!/bin/sh\n\
    echo \"curl $*\" >>\"$HOME/log\"\n\
    while [ $# -gt 0 ]; do [ \"$1\" = -o ] && out=$2; shift; done\n\
    cp \"$HOME/release.tar.gz\" \"$out\"\n";

/// A `uname` for the platform in `$FAKE_UNAME`, as `-s -m` words.
const FAKE_UNAME: &str = "#!/bin/sh\nset -- $FAKE_UNAME \"$1\"\n\
    case $3 in -s) echo \"$1\" ;; -m) echo \"$2\" ;; esac\n";

/// The herdr-core in the fake release.
const FAKE_HC: &str = "#!/bin/sh\necho 'herdr-core 0.1.16'\n";

/// A temp `HOME` whose `bin/` is the whole `PATH`: the host guards, fakes for `curl` and
/// `uname`, and only the host tools the script needs, so the owner's own herdr-core is never
/// found.
struct TestBox {
    home: tempfile::TempDir,
}

impl TestBox {
    fn new() -> Self {
        let home = tempfile::Builder::new()
            .prefix("moor-hc-install-")
            .tempdir()
            .unwrap();
        let bin = home.path().join("bin");
        moor_testkit::write_host_guards(&bin).unwrap();
        moor_testkit::write_executable(&bin.join("curl"), FAKE_CURL).unwrap();
        moor_testkit::write_executable(&bin.join("uname"), FAKE_UNAME).unwrap();
        for tool in [
            "sh",
            "tar",
            "gzip",
            "sha256sum",
            "mktemp",
            "rm",
            "mkdir",
            "mv",
            "ln",
            "cp",
        ] {
            let host = ["/usr/bin", "/bin"]
                .iter()
                .map(|d| Path::new(d).join(tool))
                .find(|p| p.exists())
                .unwrap();
            symlink(host, bin.join(tool)).unwrap();
        }
        let staged = home.path().join("stage");
        std::fs::create_dir(&staged).unwrap();
        moor_testkit::write_executable(&staged.join("herdr-core"), FAKE_HC).unwrap();
        let tar = Command::new("tar")
            .arg("-czf")
            .arg(home.path().join("release.tar.gz"))
            .arg("-C")
            .arg(&staged)
            .arg("herdr-core")
            .status()
            .unwrap();
        assert!(tar.success());
        Self { home }
    }

    fn path(&self, rel: &str) -> std::path::PathBuf {
        self.home.path().join(rel)
    }

    /// The sha256 of the fake release.
    fn sha256(&self) -> String {
        let bytes = std::fs::read(self.path("release.tar.gz")).unwrap();
        Sha256::digest(bytes)
            .iter()
            .fold(String::new(), |mut acc, b| {
                let _ = write!(acc, "{b:02x}");
                acc
            })
    }

    /// Runs `script` on a Box of platform `uname`; returns stderr and the exit code.
    fn run(&self, script: &str, uname: &str) -> (String, i32) {
        let out = Command::new(self.path("bin/sh"))
            .args(["-c", script])
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", self.path("bin"))
            .env("TMPDIR", self.home.path())
            .env("FAKE_UNAME", uname)
            .output()
            .unwrap();
        (
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        )
    }

    /// The install script for a release of the fake's checksum, or of `sha256`.
    fn script(&self, sha256: Option<&str>) -> String {
        let sha256: &'static str = Box::leak(
            sha256
                .map_or_else(|| self.sha256(), str::to_owned)
                .into_boxed_str(),
        );
        hc_install_script(&[Build {
            platform: "'Linux x86_64'",
            url: "https://releases.herdr.dev/core/0.1.16/herdr-core-linux-x86_64.tar.gz",
            sha256,
        }])
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.path("log")).unwrap_or_default()
    }
}

#[test]
fn installs_the_checked_release_and_links_herdr_core_and_hc() {
    let b = TestBox::new();
    let (err, code) = b.run(&b.script(None), "Linux x86_64");
    assert_eq!(code, 0, "{err}");
    assert!(
        b.log().contains("releases.herdr.dev/core/0.1.16/"),
        "{}",
        b.log()
    );
    for link in [".local/bin/herdr-core", ".local/bin/hc"] {
        let out = Command::new(b.path(link)).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "herdr-core 0.1.16\n");
    }
}

#[test]
fn a_download_that_fails_its_checksum_installs_nothing() {
    let b = TestBox::new();
    let (err, code) = b.run(&b.script(Some(&"0".repeat(64))), "Linux x86_64");
    assert_eq!(code, 1);
    assert!(
        err.contains("herdr-core 0.1.16 download failed its sha256 check"),
        "{err}"
    );
    assert!(!b.path(".local/bin/herdr-core").exists());
    assert!(!b.path(".local/share/moor/herdr-core/0.1.16").exists());
}

#[test]
fn a_box_with_a_running_herdr_core_downloads_nothing() {
    let b = TestBox::new();
    moor_testkit::write_executable(&b.path("bin/herdr-core"), FAKE_HC).unwrap();
    let (err, code) = b.run(&b.script(None), "Linux x86_64");
    assert_eq!(code, 0, "{err}");
    assert_eq!(b.log(), "");
}

#[test]
fn a_restored_install_directory_is_linked_without_a_download() {
    let b = TestBox::new();
    let script = b.script(None);
    assert_eq!(b.run(&script, "Linux x86_64").1, 0);
    std::fs::remove_file(b.path(".local/bin/herdr-core")).unwrap();
    std::fs::write(b.path("log"), "").unwrap();
    let (err, code) = b.run(&script, "Linux x86_64");
    assert_eq!(code, 0, "{err}");
    assert_eq!(b.log(), "");
    assert!(b.path(".local/bin/herdr-core").exists());
}

#[test]
fn a_platform_with_no_build_or_a_box_without_curl_fails_with_why() {
    let b = TestBox::new();
    let (err, code) = b.run(&b.script(None), "Darwin arm64");
    assert_eq!(code, 1);
    assert!(
        err.contains("herdr-core 0.1.16 has no build for Darwin arm64"),
        "{err}"
    );
    std::fs::remove_file(b.path("bin/curl")).unwrap();
    let (err, code) = b.run(&b.script(None), "Linux x86_64");
    assert_eq!(code, 1);
    assert!(err.contains("the Box has no curl"), "{err}");
}

#[test]
fn an_hc_already_on_the_box_is_left_alone() {
    let b = TestBox::new();
    moor_testkit::write_executable(&b.path("bin/hc"), "#!/bin/sh\necho herbstclient\n").unwrap();
    assert_eq!(b.run(&b.script(None), "Linux x86_64").1, 0);
    assert!(!b.path(".local/bin/hc").exists());
}

#[test]
fn the_pinned_builds_are_herdr_cores_own_release_files() {
    for b in &HC_BUILDS {
        assert!(
            b.url
                .starts_with(&format!("https://releases.herdr.dev/core/{HC_VERSION}/")),
            "{}",
            b.url
        );
        assert_eq!(b.sha256.len(), 64);
    }
}

#[test]
fn the_check_fails_only_for_an_install_that_does_not_run() {
    let b = TestBox::new();
    let check = hc_install().check;
    assert_eq!(b.run(&check, "Linux x86_64").1, 0, "nothing installed");
    assert_eq!(b.run(&b.script(None), "Linux x86_64").1, 0);
    assert_eq!(b.run(&check, "Linux x86_64").1, 0, "a whole install");
    std::fs::write(
        b.path(".local/share/moor/herdr-core/0.1.16/herdr-core"),
        "#!/bin/sh\nexit 1\n",
    )
    .unwrap();
    assert_ne!(b.run(&check, "Linux x86_64").1, 0, "a broken install");
}

#[test]
fn a_failed_holder_install_stops_before_the_agent_install() {
    let b = TestBox::new();
    let agent = AgentInstall {
        script: "echo agent >>\"$HOME/log\"; exit 0".to_owned(),
        dirs: vec!["~/.agent".to_owned()],
        check: "true".to_owned(),
    };
    let joined = with_agent(&HC, Some(agent.clone())).unwrap();
    assert_eq!(joined.dirs, vec![HC_DIR.to_owned(), "~/.agent".to_owned()]);
    std::fs::remove_file(b.path("bin/curl")).unwrap();
    let (err, code) = b.run(&joined.script, "Linux x86_64");
    assert_eq!(code, 1);
    assert!(err.contains("the Box has no curl"), "{err}");
    assert!(!b.log().contains("agent"), "{}", b.log());
    assert_eq!(with_agent(&TMUX, Some(agent.clone())), Some(agent));
    assert_eq!(
        with_agent(&HC, None).map(|i| i.dirs),
        Some(vec![HC_DIR.to_owned()])
    );
}
