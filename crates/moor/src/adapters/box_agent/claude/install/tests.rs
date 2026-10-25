//! The install script run under a real `sh` in a temp `HOME` with a `PATH` of fakes.

use std::os::unix::fs::symlink;
use std::path::Path;

use super::*;

/// A `curl` that leaves the installer in `-o` file, and exits 22 as well when `FAKE_CURL_FAIL` is
/// set, as a download cut off after it began does.
const FAKE_CURL: &str = "#!/bin/sh\n\
    echo \"curl $*\" >>\"$HOME/log\"\n\
    while [ $# -gt 0 ]; do [ \"$1\" = -o ] && out=$2; shift; done\n\
    cp \"$HOME/installer.sh\" \"$out\"\n\
    [ -z \"$FAKE_CURL_FAIL\" ] || exit 22\n";

/// An installer that puts a working `claude` in `~/.local/bin`, as Anthropic's does.
const INSTALLER: &str =
    "mkdir -p \"$HOME/.local/bin\" && printf '#!/bin/sh\\n' >\"$HOME/.local/bin/claude\" \
    && chmod +x \"$HOME/.local/bin/claude\" && echo ran >>\"$HOME/log\"\n";

/// An installer that finishes without leaving the agent behind.
const BROKEN_INSTALLER: &str = "echo ran >>\"$HOME/log\"\n";

/// A temp `HOME` whose `bin/` is the whole `PATH`: the host guards, a fake `curl`, and only the
/// host tools the script needs, so the owner's own `claude` is never found.
struct TestBox {
    home: tempfile::TempDir,
}

impl TestBox {
    fn new(installer: &str) -> Self {
        let home = tempfile::Builder::new()
            .prefix("moor-agent-install-")
            .tempdir()
            .unwrap();
        let bin = home.path().join("bin");
        moor_testkit::write_host_guards(&bin).unwrap();
        moor_testkit::write_executable(&bin.join("curl"), FAKE_CURL).unwrap();
        for tool in [
            "sh", "bash", "mktemp", "rm", "cp", "cat", "chmod", "mkdir", "printf", "ls", "sort",
            "tail", "ln",
        ] {
            let host = ["/usr/bin", "/bin"]
                .iter()
                .map(|d| Path::new(d).join(tool))
                .find(|p| p.exists())
                .unwrap();
            symlink(host, bin.join(tool)).unwrap();
        }
        std::fs::write(home.path().join("installer.sh"), installer).unwrap();
        Self { home }
    }

    fn without(self, tool: &str) -> Self {
        std::fs::remove_file(self.home.path().join("bin").join(tool)).unwrap();
        self
    }

    /// Runs the install script; returns stderr and the exit code.
    fn run(&self, env: &[(&str, &str)]) -> (String, i32) {
        let out = std::process::Command::new(self.home.path().join("bin/sh"))
            .args(["-c", &script()])
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", self.home.path().join("bin"))
            .env("TMPDIR", self.home.path())
            .envs(env.iter().copied())
            .output()
            .unwrap();
        (
            String::from_utf8(out.stderr).unwrap(),
            out.status.code().unwrap(),
        )
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.home.path().join("log")).unwrap_or_default()
    }

    fn has_agent(&self) -> bool {
        self.home.path().join(".local/bin/claude").exists()
    }

    /// Temp files the script left in `TMPDIR`.
    fn leftovers(&self) -> Vec<String> {
        std::fs::read_dir(self.home.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with("tmp."))
            .collect()
    }
}

#[test]
fn a_box_with_the_agent_downloads_nothing() {
    let b = TestBox::new(INSTALLER);
    let dir = b.home.path().join(".local/bin");
    std::fs::create_dir_all(&dir).unwrap();
    moor_testkit::write_executable(&dir.join("claude"), "#!/bin/sh\n").unwrap();
    assert_eq!(b.run(&[]).1, 0);
    assert_eq!(b.log(), "");
}

#[test]
fn a_box_without_the_agent_runs_the_official_installer() {
    let b = TestBox::new(INSTALLER);
    let (err, code) = b.run(&[]);
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(b.has_agent());
    let log = b.log();
    let mut lines = log.lines();
    let fetch = lines.next().unwrap();
    assert!(
        fetch.starts_with("curl -fsSL https://claude.ai/install.sh -o "),
        "{fetch}"
    );
    assert_eq!(lines.next(), Some("ran"));
    assert!(b.leftovers().is_empty());
}

#[test]
fn a_second_run_finds_the_agent_and_downloads_nothing() {
    let b = TestBox::new(INSTALLER);
    assert_eq!(b.run(&[]).1, 0);
    assert_eq!(b.run(&[]).1, 0);
    assert_eq!(b.log().lines().count(), 2);
}

#[test]
fn a_failed_download_fails_the_step_and_runs_nothing() {
    let b = TestBox::new(INSTALLER);
    assert_ne!(b.run(&[("FAKE_CURL_FAIL", "1")]).1, 0);
    assert!(!b.log().contains("ran"));
    assert!(!b.has_agent());
    assert!(b.leftovers().is_empty());
}

#[test]
fn an_installer_that_leaves_no_agent_fails_the_step() {
    let b = TestBox::new(BROKEN_INSTALLER);
    assert_ne!(b.run(&[]).1, 0);
}

#[test]
fn a_box_without_curl_says_so() {
    let b = TestBox::new(INSTALLER).without("curl");
    let (err, code) = b.run(&[]);
    assert_ne!(code, 0);
    assert!(err.contains("no curl"), "{err}");
}

#[test]
fn the_install_names_the_directory_that_holds_it() {
    assert_eq!(native().dirs, ["~/.local/share/claude"]);
}

const VERSION_BIN: &str = "#!/bin/sh\necho 1.2.3\n";

impl TestBox {
    /// Puts a cached version `name` in the install directory, as a restored cache volume does.
    fn cache_version(&self, name: &str, body: &str) {
        let dir = self.home.path().join(".local/share/claude/versions");
        std::fs::create_dir_all(&dir).unwrap();
        moor_testkit::write_executable(&dir.join(name), body).unwrap();
    }

    fn linked_to(&self) -> Option<std::path::PathBuf> {
        std::fs::read_link(self.home.path().join(".local/bin/claude")).ok()
    }
}

#[test]
fn a_restored_install_dir_is_relinked_to_the_newest_version_without_downloading() {
    let b = TestBox::new(INSTALLER);
    b.cache_version("2.9.0", VERSION_BIN);
    b.cache_version("2.10.1", VERSION_BIN);
    b.cache_version("2.10.0", VERSION_BIN);
    let (err, code) = b.run(&[]);
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(b.log(), "");
    let want = b.home.path().join(".local/share/claude/versions/2.10.1");
    assert_eq!(b.linked_to(), Some(want));
}

#[test]
fn a_cached_version_that_does_not_run_is_not_linked_and_the_installer_runs() {
    let b = TestBox::new(INSTALLER);
    b.cache_version("2.0.0", "#!/bin/sh\nexit 1\n");
    assert_eq!(b.run(&[]).1, 0);
    assert!(b.log().contains("ran"));
    assert_eq!(b.linked_to(), None);
}

/// Runs the check under `sh` with `HOME` at the box; returns its exit code.
fn check_code(b: &TestBox) -> i32 {
    std::process::Command::new(b.home.path().join("bin/sh"))
        .args(["-c", &native().check])
        .env_clear()
        .env("HOME", b.home.path())
        .env("PATH", b.home.path().join("bin"))
        .status()
        .unwrap()
        .code()
        .unwrap()
}

#[test]
fn the_check_passes_an_empty_dir_or_a_whole_install_and_fails_a_broken_one() {
    let b = TestBox::new(INSTALLER);
    assert_eq!(check_code(&b), 0);
    b.cache_version("1.0.0", VERSION_BIN);
    assert_eq!(check_code(&b), 0);
    b.cache_version("1.0.1", "#!/bin/sh\nexit 1\n");
    assert_ne!(check_code(&b), 0);
}
