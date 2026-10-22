//! The clone script runs under `sh` with real git against a local bare repo reached by `file://`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use super::{clone_script, Checkout, CloneStep};

const BRANCH: &str = "moor/demo";

/// A scratch dir, removed on drop.
struct Scratch(tempfile::TempDir);

impl Scratch {
    fn new(name: &str) -> Self {
        Self(
            tempfile::Builder::new()
                .prefix(&format!("moor-clone-{name}-"))
                .tempdir()
                .unwrap(),
        )
    }

    fn path(&self) -> &Path {
        self.0.path()
    }
}

/// A scratch dir holding a bare remote with one commit on `main`, and an empty Box workdir.
struct Remote {
    dir: Scratch,
    base: String,
}

impl Remote {
    fn new(name: &str) -> Self {
        let dir = Scratch::new(name);
        let r = Self {
            dir,
            base: String::new(),
        };
        r.git(r.dir.path(), &["init", "-q", "-b", "main", "seed"]);
        std::fs::write(r.path("seed").join("a.txt"), "one\n").unwrap();
        r.git(&r.path("seed"), &["add", "a.txt"]);
        r.git(&r.path("seed"), &["commit", "-q", "-m", "one"]);
        r.git(
            r.dir.path(),
            &["clone", "-q", "--bare", "seed", "remote.git"],
        );
        r.git(
            &r.path("remote.git"),
            &["config", "uploadpack.allowFilter", "true"],
        );
        std::fs::create_dir(r.path("box")).unwrap();
        let base = r.git(&r.path("seed"), &["rev-parse", "HEAD"]);
        Self { base, ..r }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn url(&self) -> String {
        format!("file://{}", self.path("remote.git").display())
    }

    /// Runs git in `dir` with no user or system config, and returns its trimmed stdout.
    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@x.io",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .env("HOME", self.dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// Commits a change to `a.txt` in the seed and pushes it to `refs/heads/<branch>` on the remote.
    fn push_branch(&self) -> String {
        let seed = self.path("seed");
        std::fs::write(seed.join("a.txt"), "two\n").unwrap();
        self.git(&seed, &["commit", "-q", "-am", "two"]);
        self.git(
            &seed,
            &[
                "push",
                "-q",
                "../remote.git",
                &format!("HEAD:refs/heads/{BRANCH}"),
            ],
        );
        self.git(&seed, &["rev-parse", "HEAD"])
    }

    /// Runs the clone script for `base` in the Box workdir and returns its exit code.
    fn run(&self, base: &str) -> i32 {
        let url = self.url();
        let out = self.run_with(&Checkout {
            url: &url,
            base,
            branch: BRANCH,
        });
        out.status.code().unwrap_or(-1)
    }

    /// Runs the clone script for `c` in the Box workdir.
    fn run_with(&self, c: &Checkout<'_>) -> Output {
        Command::new("sh")
            .args(["-c", &clone_script(c)])
            .current_dir(self.path("box"))
            .env("HOME", self.dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap()
    }

    /// Commits a change to `a.txt` in the Box on whatever branch it is on.
    fn commit_in_box(&self) -> String {
        let b = self.path("box");
        std::fs::write(b.join("a.txt"), "agent\n").unwrap();
        self.git(&b, &["commit", "-q", "-am", "agent"]);
        self.git(&b, &["rev-parse", "HEAD"])
    }

    /// Writes what crabbox leaves in the workdir before any command runs.
    fn crabbox_files(&self) {
        let b = self.path("box");
        std::fs::create_dir_all(b.join(".crabbox/env-x")).unwrap();
        std::fs::write(b.join(".crabbox/env-x/values.sh"), "T=placeholder\n").unwrap();
        std::fs::write(b.join(".crabbox/lease"), "x\n").unwrap();
    }

    /// Entries the clone script left under `.moor/` in the Box.
    fn moor_dir(&self) -> Vec<String> {
        std::fs::read_dir(self.path("box").join(".moor")).map_or_else(
            |_| Vec::new(),
            |d| {
                d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            },
        )
    }

    /// What a rerun must leave alone.
    fn state(&self) -> State {
        let b = self.path("box");
        State {
            branch: self.git(&b, &["symbolic-ref", "--short", "HEAD"]),
            head: self.git(&b, &["rev-parse", "HEAD"]),
            refs: self.git(&b, &["for-each-ref"]),
            reflog: self.git(&b, &["reflog", "show", "HEAD"]),
            status: self.git(&b, &["status", "--porcelain"]),
            exclude: std::fs::read_to_string(b.join(".git/info/exclude")).unwrap(),
        }
    }
}

/// The Box checkout as a rerun of the script could change it.
#[derive(Debug, PartialEq, Eq)]
struct State {
    branch: String,
    head: String,
    refs: String,
    reflog: String,
    status: String,
    exclude: String,
}

#[test]
fn the_first_run_is_a_partial_clone_on_the_box_branch_at_the_base() {
    let r = Remote::new("first");
    assert_eq!(r.run(&r.base), 0);
    let b = r.path("box");
    assert_eq!(r.git(&b, &["rev-parse", "HEAD"]), r.base);
    assert_eq!(r.git(&b, &["remote", "get-url", "origin"]), r.url());
    assert_eq!(
        r.git(&b, &["config", "remote.origin.partialclonefilter"]),
        "blob:none"
    );
    assert_eq!(std::fs::read_to_string(b.join("a.txt")).unwrap(), "one\n");
    assert_eq!(r.state().branch, BRANCH);
}

#[test]
fn a_second_run_changes_nothing() {
    let r = Remote::new("twice");
    assert_eq!(r.run(&r.base), 0);
    let b = r.path("box");
    std::fs::create_dir_all(b.join(".moor")).unwrap();
    std::fs::write(b.join(".moor/setup.log"), "x").unwrap();
    let first = r.state();
    assert_eq!(first.status, "", ".moor/ is excluded");
    assert_eq!(r.run(&r.base), 0);
    assert_eq!(r.state(), first);
}

#[test]
fn a_workdir_holding_crabbox_files_is_cloned_and_they_stay_untracked() {
    let r = Remote::new("crabbox");
    r.crabbox_files();
    assert_eq!(r.run(&r.base), 0);
    let b = r.path("box");
    let first = r.state();
    assert_eq!(first.branch, BRANCH);
    assert_eq!(first.head, r.base);
    assert_eq!(first.status, "", ".crabbox/ is excluded");
    r.git(&b, &["check-ignore", "-q", ".crabbox/env-x/values.sh"]);
    assert_eq!(
        std::fs::read_to_string(b.join(".crabbox/env-x/values.sh")).unwrap(),
        "T=placeholder\n"
    );
    assert_eq!(r.moor_dir(), Vec::<String>::new(), "no temp dir is left");
    assert_eq!(r.run(&r.base), 0);
    assert_eq!(r.state(), first);
}

#[test]
fn a_failed_clone_beside_crabbox_files_leaves_no_temp_dir() {
    let r = Remote::new("cleanup");
    r.crabbox_files();
    let url = format!("file://{}", r.path("missing.git").display());
    let out = r.run_with(&Checkout {
        url: &url,
        base: &r.base,
        branch: BRANCH,
    });
    assert_eq!(
        out.status.code().and_then(CloneStep::of_exit),
        Some(CloneStep::Clone),
        "{out:?}"
    );
    assert_eq!(r.moor_dir(), Vec::<String>::new());
    assert!(!r.path("box").join(".git").exists());
    assert_eq!(r.run(&r.base), 0, "a retry with the right URL clones");
}

#[test]
fn a_branch_on_the_remote_is_checked_out_instead_of_the_base() {
    let r = Remote::new("branch");
    let tip = r.push_branch();
    assert_ne!(tip, r.base);
    assert_eq!(r.run(&r.base), 0);
    let b = r.path("box");
    assert_eq!(r.git(&b, &["symbolic-ref", "--short", "HEAD"]), BRANCH);
    assert_eq!(r.git(&b, &["rev-parse", "HEAD"]), tip);
    assert_eq!(std::fs::read_to_string(b.join("a.txt")).unwrap(), "two\n");
}

#[test]
fn a_clone_stopped_before_its_checkout_carries_on() {
    let r = Remote::new("resume");
    let b = r.path("box");
    r.git(&b, &["clone", "-q", "--no-checkout", &r.url(), "."]);
    assert_eq!(r.run(&r.base), 0);
    assert_eq!(r.git(&b, &["symbolic-ref", "--short", "HEAD"]), BRANCH);
    assert_eq!(r.git(&b, &["rev-parse", "HEAD"]), r.base);
}

#[test]
fn a_rerun_after_head_left_the_box_branch_keeps_its_commits() {
    let r = Remote::new("kept");
    r.crabbox_files();
    assert_eq!(r.run(&r.base), 0);
    let b = r.path("box");
    let agent = r.commit_in_box();
    r.git(&b, &["checkout", "-q", "-b", "elsewhere", &r.base]);
    assert_eq!(r.run(&r.base), 0);
    assert_eq!(r.git(&b, &["symbolic-ref", "--short", "HEAD"]), BRANCH);
    assert_eq!(r.git(&b, &["rev-parse", "HEAD"]), agent);
}

#[test]
fn a_base_the_clone_lacks_is_fetched_from_the_remote() {
    let r = Remote::new("fetch");
    let b = r.path("box");
    r.git(&b, &["clone", "-q", "--no-checkout", &r.url(), "."]);
    let seed = r.path("seed");
    std::fs::write(seed.join("a.txt"), "later\n").unwrap();
    r.git(&seed, &["commit", "-q", "-am", "later"]);
    r.git(
        &seed,
        &["push", "-q", "../remote.git", "HEAD:refs/heads/main"],
    );
    let later = r.git(&seed, &["rev-parse", "HEAD"]);
    assert_eq!(r.run(&later), 0);
    assert_eq!(r.git(&b, &["rev-parse", "HEAD"]), later);
}

#[test]
fn a_branch_git_cannot_check_out_stops_at_the_checkout_step_with_its_reason() {
    let r = Remote::new("badbranch");
    let url = r.url();
    let c = Checkout {
        url: &url,
        base: &r.base,
        branch: "a..b",
    };
    let out = r.run_with(&c);
    let step = out.status.code().and_then(CloneStep::of_exit);
    assert_eq!(step, Some(CloneStep::Checkout), "{out:?}");
    assert_eq!(
        CloneStep::Checkout.reason(&c),
        "could not check out branch a..b"
    );
}

#[test]
fn an_option_like_url_base_or_branch_is_refused_before_git_runs() {
    let r = Remote::new("dash");
    let url = r.url();
    let pwn = r.path("pwned");
    let upload = format!("--upload-pack=touch {}", pwn.display());
    let cases = [
        (
            upload.as_str(),
            r.base.as_str(),
            BRANCH,
            CloneStep::Clone,
            "repo URL",
        ),
        (
            url.as_str(),
            upload.as_str(),
            BRANCH,
            CloneStep::Base,
            "base",
        ),
        (
            url.as_str(),
            r.base.as_str(),
            "-x",
            CloneStep::Checkout,
            "branch",
        ),
    ];
    for (url, base, branch, step, what) in cases {
        let out = r.run_with(&Checkout { url, base, branch });
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.code().and_then(CloneStep::of_exit),
            Some(step),
            "{out:?}"
        );
        assert!(
            stderr.contains(&format!("the {what} starts with -")),
            "{stderr}"
        );
        assert!(!pwn.exists(), "git ran {upload}");
    }
}

#[test]
fn a_base_that_is_not_on_the_remote_stops_at_the_base_step() {
    let r = Remote::new("nobase");
    let missing = "0123456789abcdef0123456789abcdef01234567";
    let code = r.run(missing);
    assert_eq!(
        CloneStep::of_exit(code),
        Some(CloneStep::Base),
        "exit {code}"
    );
}

#[test]
fn an_unreachable_url_stops_at_the_clone_step() {
    let r = Remote::new("unreachable");
    let url = format!("file://{}", r.path("missing.git").display());
    let out = r.run_with(&Checkout {
        url: &url,
        base: &r.base,
        branch: BRANCH,
    });
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code().and_then(CloneStep::of_exit),
        Some(CloneStep::Clone),
        "{out:?}"
    );
    assert!(stderr.contains("missing.git"), "{stderr}");
}

#[test]
fn values_are_quoted() {
    let script = clone_script(&Checkout {
        url: "https://github.com/o/n.git",
        base: "$(touch x)",
        branch: "b'; rm -rf /; '",
    });
    assert!(script.contains("'$(touch x)'"), "{script}");
    assert!(script.contains(r"'b'\''; rm -rf /; '\'''"), "{script}");
}

/// Runs the clone script with a `git` on `PATH` that records where `git clone` writes and, when
/// `race` is set, has another run's `.git` appear in the workdir before this run moves its own.
fn run_beside_another_clone(r: &Remote, race: bool) -> (Output, PathBuf) {
    let path = std::env::var("PATH").unwrap();
    let real = std::env::split_paths(&path)
        .map(|d| d.join("git"))
        .find(|g| g.is_file())
        .unwrap();
    let bin = r.path("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = clone ]; then\n\
         \x20 for last; do :; done\n\
         \x20 printf '%s' \"$last\" > {log}\n\
         \x20 {real} \"$@\" || exit\n\
         \x20 {race}\n\
         \x20 exit 0\n\
         fi\n\
         exec {real} \"$@\"\n",
        log = r.path("clone-into").display(),
        real = real.display(),
        race = if race {
            format!("{} init -q .", real.display())
        } else {
            ":".to_owned()
        },
    );
    moor_testkit::write_executable(&bin.join("git"), fake).unwrap();
    let url = r.url();
    let script = clone_script(&Checkout {
        url: &url,
        base: &r.base,
        branch: BRANCH,
    });
    let out = Command::new("sh")
        .args(["-c", &script])
        .current_dir(r.path("box"))
        .env("HOME", r.dir.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("PATH", format!("{}:{path}", bin.display()))
        .output()
        .unwrap();
    let into = std::fs::read_to_string(r.path("clone-into")).unwrap();
    (out, r.path("box").join(into))
}

#[test]
fn the_clone_goes_into_a_temp_dir_under_the_workdir_so_the_rename_stays_on_one_filesystem() {
    let r = Remote::new("samefs");
    r.crabbox_files();
    let (out, into) = run_beside_another_clone(&r, false);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(
        into.starts_with(r.path("box").join(".moor")),
        "cloned into {}",
        into.display()
    );
    assert_eq!(r.state().branch, BRANCH);
}

#[test]
fn a_git_dir_that_appears_while_cloning_is_never_nested_and_stops_at_the_clone_step() {
    let r = Remote::new("race");
    r.crabbox_files();
    let (out, _) = run_beside_another_clone(&r, true);
    assert_eq!(
        out.status.code().and_then(CloneStep::of_exit),
        Some(CloneStep::Clone),
        "{out:?}"
    );
    let b = r.path("box");
    assert!(!b.join(".git/.git").exists(), "nested .git");
    assert!(b.join(".git/HEAD").is_file(), "the other run's .git stays");
    assert_eq!(r.moor_dir(), Vec::<String>::new(), "no temp dir is left");
}
