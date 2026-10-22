//! Tests for the recorded-run script; each runs the generated text under `sh` in a scratch git repo.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{head_script, parse_head, record_script, HEAD_FILE};
use crate::Sha;

const ZERO: &str = "0000000000000000000000000000000000000000";

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

struct Out {
    code: i32,
    stderr: String,
}

/// Scratch directory, removed on drop.
struct Sandbox {
    root: PathBuf,
    _dir: tempfile::TempDir,
}

impl Sandbox {
    fn new(name: &str, git: bool) -> Self {
        let dir = tempfile::Builder::new()
            .prefix(&format!("moor-record-{name}-"))
            .tempdir()
            .unwrap();
        let sb = Self {
            root: dir.path().to_path_buf(),
            _dir: dir,
        };
        if git {
            sb.git(&["init", "-q"]);
            sb.git(&["config", "user.name", "t"]);
            sb.git(&["config", "user.email", "t@example.com"]);
        }
        sb
    }

    fn git(&self, args: &[&str]) {
        let st = Command::new("git")
            .args(args)
            .current_dir(&self.root)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?}");
    }

    fn write(&self, rel: &str, content: &str) {
        let p = self.root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn commit(&self) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", "x"]);
    }

    fn exists(&self, rel: &str) -> bool {
        self.root.join(rel).exists()
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.root.join(rel)).unwrap_or_default()
    }

    fn ignore(&self, lines: &str) {
        self.write(".gitignore", lines);
    }

    /// The checkout's HEAD, or an all-zero sha when it has none.
    fn head(&self) -> Sha {
        let o = Command::new("git")
            .args(["rev-parse", "--verify", "HEAD"])
            .current_dir(&self.root)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&o.stdout).trim().to_owned();
        Sha::parse(if o.status.success() { &text } else { ZERO }).unwrap()
    }

    fn run(&self, setup: &[&str], cmd: &str) -> Out {
        self.run_at(setup, cmd, &self.head())
    }

    fn run_at(&self, setup: &[&str], cmd: &str, head: &Sha) -> Out {
        self.script(&record_script(&strings(setup), cmd, head.as_str()), None)
    }

    fn run_in(&self, setup: &[&str], cmd: &str, path: Option<&Path>) -> Out {
        self.script(
            &record_script(&strings(setup), cmd, self.head().as_str()),
            path,
        )
    }

    fn script(&self, text: &str, path: Option<&Path>) -> Out {
        let mut c = Command::new("/bin/sh");
        c.arg("-c").arg(text).current_dir(&self.root);
        if let Some(p) = path {
            c.env("PATH", p);
        }
        let o = c.output().unwrap();
        Out {
            code: o.status.code().unwrap(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    /// One passing run that creates `dir`, then a second run; true when `dir` survived it.
    fn kept_after_pass(&self, lock: &str, dir: &str) -> bool {
        self.write(lock, "{}\n");
        self.commit();
        let first = self.run(&[], &format!("mkdir -p '{dir}' && touch '{dir}/f'"));
        assert_eq!(first.code, 0);
        assert_eq!(self.run(&[], "true").code, 0);
        self.exists(&format!("{dir}/f"))
    }
}

#[test]
fn untracked_files_are_removed_and_moor_dir_is_kept() {
    let sb = Sandbox::new("clean", true);
    sb.write("tracked.txt", "t");
    sb.commit();
    sb.write("junk.txt", "j");
    assert_eq!(sb.run(&[], "touch .moor/mark").code, 0);
    assert!(!sb.exists("junk.txt"));
    assert!(sb.exists("tracked.txt"));
    assert!(sb.exists(".moor/mark"));
}

#[test]
fn outside_a_git_checkout_fails_without_running_the_command() {
    let sb = Sandbox::new("nogit", false);
    let out = sb.run(&[], "touch ran");
    assert_ne!(out.code, 0);
    assert!(out
        .stderr
        .contains("moor: --record needs a git checkout on the Box"));
    assert!(!sb.exists("ran"));
}

#[test]
fn command_exit_code_is_returned() {
    let sb = Sandbox::new("rc", true);
    sb.write("a", "a");
    sb.commit();
    assert_eq!(sb.run(&[], "exit 3").code, 3);
}

/// Node dirs and the tool caches kept on a matching key; other `.next` content is not.
#[test]
fn node_dirs_and_tool_caches_are_kept_on_matching_key() {
    let sb = Sandbox::new("node", true);
    sb.write("package-lock.json", "{}\n");
    sb.write(".nvmrc", "20\n");
    sb.commit();
    let make = "mkdir -p node_modules/x web/node_modules/y .next/cache .next/server .turbo .nx/cache \
                && touch node_modules/x/f web/node_modules/y/f .next/cache/c .next/server/s .turbo/t .nx/cache/n";
    assert_eq!(sb.run(&[], make).code, 0);
    assert_eq!(sb.run(&[], "true").code, 0);
    for kept in [
        "node_modules/x/f",
        "web/node_modules/y/f",
        ".next/cache/c",
        ".turbo/t",
        ".nx/cache/n",
    ] {
        assert!(sb.exists(kept), "{kept}");
    }
    assert!(!sb.exists(".next/server/s"));
}

#[test]
fn python_venv_is_kept_on_matching_key() {
    assert!(Sandbox::new("uv", true).kept_after_pass("uv.lock", ".venv"));
    assert!(Sandbox::new("poetry", true).kept_after_pass("poetry.lock", ".venv"));
    assert!(Sandbox::new("pipenv", true).kept_after_pass("Pipfile.lock", ".venv"));
}

#[test]
fn bundler_dir_is_kept_on_matching_key() {
    assert!(Sandbox::new("bundler", true).kept_after_pass("Gemfile.lock", "vendor/bundle"));
}

#[test]
fn composer_vendor_is_kept_on_matching_key() {
    assert!(Sandbox::new("composer", true).kept_after_pass("composer.lock", "vendor"));
}

#[test]
fn pods_dir_is_kept_on_matching_key() {
    assert!(Sandbox::new("pods", true).kept_after_pass("Podfile.lock", "Pods"));
}

#[test]
fn nested_lockfile_keeps_its_own_dir() {
    assert!(Sandbox::new("nested", true).kept_after_pass("app/uv.lock", "app/.venv"));
}

/// Passing run, then `change`, then a second run; true when `node_modules` survived.
fn survives_change(name: &str, setup2: &[&str], change: impl Fn(&Sandbox)) -> bool {
    let sb = Sandbox::new(name, true);
    sb.ignore("node_modules/\n");
    sb.write("package-lock.json", "{}\n");
    sb.write(".nvmrc", "20\n");
    sb.commit();
    assert_eq!(
        sb.run(&["true"], "mkdir node_modules && touch node_modules/f")
            .code,
        0
    );
    change(&sb);
    sb.commit();
    assert_eq!(sb.run(setup2, "true").code, 0);
    sb.exists("node_modules/f")
}

#[test]
fn unchanged_inputs_keep_the_dir() {
    assert!(survives_change("same", &["true"], |_| {}));
}

#[test]
fn lockfile_change_invalidates_the_key() {
    assert!(!survives_change("lockchg", &["true"], |sb| sb
        .write("package-lock.json", "{\"a\":1}\n")));
}

#[test]
fn version_pin_change_invalidates_the_key() {
    assert!(!survives_change("pinchg", &["true"], |sb| sb.write(".nvmrc", "22\n")));
}

#[test]
fn setup_change_invalidates_the_key() {
    assert!(!survives_change("setupchg", &["true", "true"], |_| {}));
}

/// Tracked lockfile that must not keep anything; the command still runs.
fn keeps_nothing(sb: &Sandbox, dir: &str) -> Out {
    sb.commit();
    let make = format!("mkdir -p '{dir}' && touch '{dir}/f'");
    assert_eq!(sb.run(&[], &make).code, 0);
    let out = sb.run(&[], "touch .moor/ran");
    assert_eq!(out.code, 0);
    assert!(sb.exists(".moor/ran"));
    assert!(!sb.exists(&format!("{dir}/f")), "{dir} was kept");
    out
}

#[test]
fn vendored_lockfile_keeps_nothing() {
    let sb = Sandbox::new("vendored", true);
    sb.write("vendor/pkg/package-lock.json", "{}\n");
    keeps_nothing(&sb, "vendor/pkg/node_modules");
}

#[test]
fn lockfile_path_with_space_keeps_nothing() {
    let sb = Sandbox::new("space", true);
    sb.write("my app/package-lock.json", "{}\n");
    keeps_nothing(&sb, "my app/node_modules");
}

#[test]
fn lockfile_without_cache_entry_keeps_nothing() {
    let sb = Sandbox::new("unknown", true);
    sb.write("Cargo.lock", "x\n");
    keeps_nothing(&sb, "target");
}

#[test]
fn more_than_64_lockfiles_keep_nothing_and_warn() {
    let sb = Sandbox::new("many", true);
    for i in 0..65 {
        sb.write(&format!("d{i}/package-lock.json"), "{}\n");
    }
    let out = keeps_nothing(&sb, "d0/node_modules");
    assert!(out.stderr.contains("more than 64 lockfiles"));
}

fn find_on_path(tool: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(tool))
        .find(|p| p.is_file())
}

/// Directory of symlinks to the tools the script needs, minus every hash tool.
fn bin_without_hash_tools(sb: &Sandbox) -> PathBuf {
    let bin = sb.root.with_extension("bin");
    let _ = fs::remove_dir_all(&bin);
    fs::create_dir_all(&bin).unwrap();
    let tools = [
        "sh", "bash", "git", "find", "rm", "mkdir", "cat", "wc", "tr", "sed", "dirname", "uname",
        "id", "mv", "touch", "true", "sudo", "grep",
    ];
    for t in tools {
        if let Some(src) = find_on_path(t) {
            std::os::unix::fs::symlink(src, bin.join(t)).unwrap();
        }
    }
    bin
}

#[test]
fn missing_hash_tool_keeps_nothing_and_warns() {
    let sb = Sandbox::new("nohash", true);
    sb.write("package-lock.json", "{}\n");
    sb.commit();
    let bin = bin_without_hash_tools(&sb);
    let first = sb.run_in(
        &[],
        "mkdir node_modules && touch node_modules/f",
        Some(&bin),
    );
    assert_eq!(first.code, 0, "{}", first.stderr);
    let second = sb.run_in(&[], "touch .moor/ran", Some(&bin));
    let _ = fs::remove_dir_all(&bin);
    assert_eq!(second.code, 0, "{}", second.stderr);
    assert!(second.stderr.contains("no sha256sum, shasum or openssl"));
    assert!(sb.exists(".moor/ran"));
    assert!(!sb.exists("node_modules/f"));
}

#[test]
fn failing_command_leaves_the_key_unpromoted() {
    let sb = Sandbox::new("fail", true);
    sb.write("package-lock.json", "{}\n");
    sb.commit();
    assert_eq!(
        sb.run(&[], "mkdir node_modules && touch node_modules/f; exit 1")
            .code,
        1
    );
    assert_eq!(sb.run(&[], "true").code, 0);
    assert!(!sb.exists("node_modules/f"));
}

#[test]
fn passing_command_promotes_the_key() {
    let sb = Sandbox::new("pass", true);
    sb.write("package-lock.json", "{}\n");
    sb.commit();
    assert_eq!(
        sb.run(&[], "mkdir node_modules && touch node_modules/f")
            .code,
        0
    );
    assert!(sb.exists(".moor/cache/package-lock.json.key"));
    assert!(!sb.exists(".moor/cache/package-lock.json.next"));
}

#[test]
fn setup_runs_again_after_every_clean() {
    let sb = Sandbox::new("setup", true);
    sb.write("a", "a");
    sb.commit();
    let setup = ["echo x >> .moor/count"];
    assert_eq!(sb.run(&setup, "true").code, 0);
    assert_eq!(sb.run(&setup, "true").code, 0);
    assert_eq!(sb.read(".moor/count").lines().count(), 2);
}

#[test]
fn failing_setup_stops_before_the_command() {
    let sb = Sandbox::new("badsetup", true);
    sb.write("a", "a");
    sb.commit();
    let out = sb.run(&["exit 7"], "touch .moor/ran");
    assert_eq!(out.code, 7);
    assert!(out
        .stderr
        .contains("moor: --record fresh-state prep failed (exit 7)"));
    assert!(!sb.exists(".moor/ran"));
}

fn setup_sandbox(name: &str) -> Sandbox {
    let sb = Sandbox::new(name, true);
    sb.write("a", "a");
    sb.commit();
    sb
}

#[test]
fn first_failing_step_skips_later_steps_and_the_command() {
    let sb = setup_sandbox("stepfail");
    let out = sb.run(&["false", "echo c >> setup_ran"], "touch ran");
    assert_ne!(out.code, 0);
    assert!(!sb.exists("setup_ran"));
    assert!(!sb.exists("ran"));
}

#[test]
fn and_list_failure_still_stops_setup() {
    let sb = setup_sandbox("andlist");
    let out = sb.run(&["false && true", "touch second"], "touch ran");
    assert_ne!(out.code, 0);
    assert!(!sb.exists("second"));
    assert!(!sb.exists("ran"));
}

#[test]
fn exit_zero_in_a_step_does_not_skip_later_steps() {
    let sb = setup_sandbox("exitzero");
    let out = sb.run(&["exit 0", "touch second"], "touch ran");
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(sb.exists("second"));
    assert!(sb.exists("ran"));
}

#[test]
fn disabling_errexit_in_a_step_does_not_leak_to_later_steps() {
    let sb = setup_sandbox("seteoff");
    let out = sb.run(&["set +e", "false"], "touch ran");
    assert_ne!(out.code, 0);
    assert!(!sb.exists("ran"));
}

#[test]
fn step_with_unbalanced_parens_cannot_break_out() {
    let sb = setup_sandbox("parens");
    let out = sb.run(&[") ; touch escaped ; (", "true"], "touch ran");
    assert_ne!(out.code, 0);
    assert!(!sb.exists("escaped"));
    assert!(!sb.exists("ran"));
}

/// A committed checkout with `kept.txt` and `gone.txt` tracked and `*.log` ignored.
fn probe_sandbox(name: &str) -> Sandbox {
    let sb = Sandbox::new(name, true);
    sb.ignore("*.log\n");
    sb.write("kept.txt", "k");
    sb.write("gone.txt", "g");
    sb.commit();
    sb
}

/// The head probe's exit code and the `(head, dirty)` it wrote.
fn probe(sb: &Sandbox) -> (i32, Option<(String, u32)>) {
    let out = sb.script(&head_script(), None);
    (out.code, parse_head(&sb.read(HEAD_FILE)))
}

fn probed_dirty(sb: &Sandbox) -> u32 {
    let (code, got) = probe(sb);
    assert_eq!(code, 0);
    let (head, dirty) = got.expect("the probe wrote a head line");
    assert_eq!(head, sb.head().as_str());
    dirty
}

#[test]
fn head_probe_counts_a_clean_checkout_as_clean() {
    assert_eq!(probed_dirty(&probe_sandbox("p-clean")), 0);
}

#[test]
fn head_probe_counts_a_modified_file() {
    let sb = probe_sandbox("p-mod");
    sb.write("kept.txt", "changed");
    assert_eq!(probed_dirty(&sb), 1);
}

#[test]
fn head_probe_counts_an_untracked_file() {
    let sb = probe_sandbox("p-new");
    sb.write("new.txt", "n");
    assert_eq!(probed_dirty(&sb), 1);
}

#[test]
fn head_probe_skips_an_ignored_file() {
    let sb = probe_sandbox("p-ign");
    sb.write("build.log", "l");
    assert_eq!(probed_dirty(&sb), 0);
}

#[test]
fn head_probe_counts_a_staged_delete() {
    let sb = probe_sandbox("p-del");
    sb.git(&["rm", "-q", "gone.txt"]);
    assert_eq!(probed_dirty(&sb), 1);
}

#[test]
fn head_probe_counts_a_file_name_with_a_space() {
    let sb = probe_sandbox("p-space");
    sb.write("a b.txt", "s");
    assert_eq!(probed_dirty(&sb), 1);
}

#[test]
fn head_probe_fails_and_writes_nothing_without_a_commit() {
    let sb = Sandbox::new("p-empty", true);
    let (code, got) = probe(&sb);
    assert_ne!(code, 0);
    assert_eq!(got, None);
}

#[test]
fn parse_head_takes_only_an_exact_head_line() {
    let sha = "a".repeat(40);
    assert_eq!(
        parse_head(&format!("head={sha} dirty=3\n")),
        Some((sha.clone(), 3))
    );
    for bad in [
        String::new(),
        format!("head={} dirty=0", "a".repeat(39)),
        format!("head={sha} dirty=-1"),
        format!("head={sha} dirty="),
        format!("head={sha} dirty=0 extra"),
        format!("dirty=0 head={sha}"),
    ] {
        assert_eq!(parse_head(&bad), None, "{bad:?}");
    }
}

mod moved;
