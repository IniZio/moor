//! A bare origin served over `file://`, a seed clone that pushes to it, and a login home whose
//! `~/moor/repos/r` is the Box's work tree. Scripts run with the host guards first on `PATH`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The Box branch the fixtures use.
pub(crate) const BRANCH: &str = "moor/demo";

/// A temp dir holding `seed/`, `remote.git` and `home/`, removed on drop.
pub(crate) struct Origin {
    /// The temp dir.
    dir: tempfile::TempDir,
    /// The first commit on `main`.
    pub(crate) base: String,
}

impl Origin {
    /// A remote whose `main` holds `a.txt`, `b.txt` and a `.gitignore` of `build/`.
    pub(crate) fn new(name: &str) -> Self {
        let dir = tempfile::Builder::new()
            .prefix(&format!("moor-git-{name}-"))
            .tempdir()
            .unwrap();
        let mut o = Self {
            dir,
            base: String::new(),
        };
        moor_testkit::write_host_guards(&o.home().join("bin")).unwrap();
        let config = "[protocol \"file\"]\n\tallow = always\n[commit]\n\tgpgsign = false\n";
        std::fs::write(o.home().join(".gitconfig"), config).unwrap();
        std::fs::create_dir_all(o.tree()).unwrap();
        o.git(o.dir.path(), &["init", "-q", "-b", "main", "seed"]);
        let seed = o.seed();
        std::fs::write(seed.join("a.txt"), "one\n").unwrap();
        std::fs::write(seed.join("b.txt"), "unchanged\n").unwrap();
        std::fs::write(seed.join(".gitignore"), "build/\n").unwrap();
        o.git(&seed, &["add", "."]);
        o.git(&seed, &["commit", "-q", "-m", "one"]);
        o.git(
            o.dir.path(),
            &["clone", "-q", "--bare", "seed", "remote.git"],
        );
        o.git(
            &o.path("remote.git"),
            &["config", "uploadpack.allowFilter", "true"],
        );
        o.git(
            &o.path("remote.git"),
            &["config", "uploadpack.allowAnySHA1InWant", "true"],
        );
        o.git(&seed, &["remote", "add", "origin", "../remote.git"]);
        o.base = o.git(&seed, &["rev-parse", "HEAD"]);
        o
    }

    /// `rel` inside the temp dir.
    pub(crate) fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    /// The seed clone.
    pub(crate) fn seed(&self) -> PathBuf {
        self.path("seed")
    }

    /// The login home.
    pub(crate) fn home(&self) -> PathBuf {
        self.path("home")
    }

    /// The Box's work tree, `~/moor/repos/r`.
    pub(crate) fn tree(&self) -> PathBuf {
        self.home().join("moor/repos/r")
    }

    /// The origin's URL.
    pub(crate) fn url(&self) -> String {
        format!("file://{}", self.path("remote.git").display())
    }

    /// The env every git and script run gets: the temp home, whose `.gitconfig` allows file
    /// transport for submodules, no host config, and the host guards first on `PATH`.
    fn env(&self, cmd: &mut Command) {
        let path = format!(
            "{}:{}",
            self.home().join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        cmd.env("HOME", self.home())
            .env("PATH", path)
            .env("XDG_CONFIG_HOME", self.home().join(".config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@x.io")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@x.io")
            .env("DOCKER_HOST", "unix:///nonexistent/moor-test")
            .env("CONTAINER_HOST", "unix:///nonexistent/moor-test");
    }

    /// Runs git in `dir`, asserts it succeeded and returns its trimmed stdout.
    pub(crate) fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = self.try_git(dir, args);
        assert!(out.status.success(), "git {args:?} in {dir:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// Runs git in `dir`.
    pub(crate) fn try_git(&self, dir: &Path, args: &[&str]) -> Output {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(dir);
        self.env(&mut cmd);
        cmd.output().unwrap()
    }

    /// Runs `script` under `sh` in `dir`, which must lie inside the temp dir, since the scripts
    /// under test may empty their working directory.
    pub(crate) fn sh(&self, dir: &Path, script: &str, vars: &[(&str, &str)]) -> Output {
        let real = dir.canonicalize().unwrap();
        let root = self.dir.path().canonicalize().unwrap();
        assert!(
            real.starts_with(&root) && real != root,
            "{real:?} is outside {root:?}"
        );
        let mut cmd = Command::new("sh");
        cmd.args(["-c", script]).current_dir(dir);
        self.env(&mut cmd);
        cmd.envs(vars.iter().copied());
        cmd.output().unwrap()
    }

    /// Writes `files` in the seed, commits them and pushes the commit to `branch`; returns it.
    pub(crate) fn push(&self, branch: &str, files: &[(&str, &str)]) -> String {
        let seed = self.seed();
        for (f, body) in files {
            let p = seed.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        self.git(&seed, &["add", "-A"]);
        self.git(&seed, &["commit", "-q", "--allow-empty", "-m", "change"]);
        self.git(
            &seed,
            &[
                "push",
                "-q",
                "-f",
                "origin",
                &format!("HEAD:refs/heads/{branch}"),
            ],
        );
        self.git(&seed, &["rev-parse", "HEAD"])
    }

    /// The entries of `dir`, sorted.
    pub(crate) fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|d| {
                d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}
