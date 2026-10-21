//! Tests for the Claude Code runner against a fake `claude` script that records what it got.
#![allow(clippy::panic, reason = "test assertions")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use moor_core::secret::Secret;

use super::{instructions, run, Claude, HostEnv};
use crate::coordinator::Agent as _;
use crate::credentials::{CheckError, CredentialCheck as _};

/// The fake agent the daemon tests use too.
const FAKE: &str = include_str!("../../../tests/support/fake-claude.sh");

/// The fake, written once for this test binary; each test links to this one file.
fn shared_fake() -> &'static Path {
    static FAKE_PATH: OnceLock<PathBuf> = OnceLock::new();
    FAKE_PATH.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap().keep();
        let path = dir.join("fake-claude");
        moor_testkit::write_executable(&path, FAKE).unwrap();
        path
    })
}

/// Links the fake `claude` into `dir`, where it keeps its records, printing `version` for
/// `--version` when given.
pub(crate) fn install(dir: &Path, version: Option<&str>) -> PathBuf {
    let path = dir.join("claude");
    std::os::unix::fs::symlink(shared_fake(), &path).unwrap();
    if let Some(version) = version {
        std::fs::write(dir.join("version"), version).unwrap();
    }
    path
}

fn host() -> HostEnv {
    HostEnv {
        home: Some("/home/op".into()),
        user: Some("op".into()),
    }
}

/// A probed fake in `bin` and an empty config dir in `cfg`.
fn claude(bin: &Path) -> Claude {
    Claude::probe(&install(bin, None), host()).unwrap()
}

fn recorded(bin: &Path, file: &str) -> String {
    std::fs::read_to_string(bin.join("run-0").join(file)).unwrap()
}

#[test]
fn the_version_gate_takes_2_1_304_and_newer() {
    for (printed, ok) in [
        ("2.1.304 (Claude Code)", true),
        ("2.2.0 (Claude Code)", true),
        ("10.0.0", true),
        ("2.1.303 (Claude Code)", false),
        ("1.99.999", false),
    ] {
        let bin = tempfile::tempdir().unwrap();
        let got = Claude::probe(&install(bin.path(), Some(printed)), host());
        assert_eq!(got.is_ok(), ok, "{printed}: {got:?}");
    }
    let bin = tempfile::tempdir().unwrap();
    let old = Claude::probe(&install(bin.path(), Some("2.1.303")), host()).unwrap_err();
    assert_eq!(old, "Claude Code 2.1.303 is older than 2.1.304");
    let bin = tempfile::tempdir().unwrap();
    let probed = Claude::probe(&install(bin.path(), Some("2.1.305 (Claude Code)")), host());
    assert_eq!(probed.unwrap().version(), "2.1.305");
}

#[test]
fn a_missing_or_silent_binary_is_refused_by_path() {
    let bin = tempfile::tempdir().unwrap();
    let missing = bin.path().join("nope");
    let err = Claude::probe(&missing, host()).unwrap_err();
    assert!(err.starts_with(&missing.display().to_string()), "{err}");
    let err = Claude::probe(&install(bin.path(), Some("hello")), host()).unwrap_err();
    assert!(err.ends_with("--version printed no version"), "{err}");
}

#[test]
fn the_check_puts_the_token_on_fd_3_only() {
    let bin = tempfile::tempdir().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let token = "sk-ant-oat01-good-7f3a9c1e";
    claude(bin.path())
        .check(cfg.path(), &Secret::new(token.into()))
        .unwrap();
    assert_eq!(recorded(bin.path(), "fd3"), token);
    let argv = recorded(bin.path(), "argv");
    let want = "-p\n--no-session-persistence\n--model\nhaiku\n--output-format\nstream-json\n\
                --verbose\n--tools\n\n--setting-sources\n\n--disable-slash-commands\n\
                --strict-mcp-config\n--permission-prompts\nnone\n";
    assert_eq!(argv, want);
    assert_eq!(recorded(bin.path(), "stdin"), "Reply OK.");
    let mut env: Vec<String> = recorded(bin.path(), "env")
        .lines()
        .map(str::to_owned)
        .collect();
    env.sort();
    let cfg_dir = cfg.path().display();
    let want = [
        "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR=3".to_owned(),
        format!("CLAUDE_CONFIG_DIR={cfg_dir}"),
        "HOME=/home/op".to_owned(),
        "LANG=C.UTF-8".to_owned(),
        "PATH=/usr/local/bin:/usr/bin:/bin".to_owned(),
        "USER=op".to_owned(),
    ];
    assert_eq!(env, want);
    assert_eq!(recorded(bin.path(), "cwd").trim_end(), cfg_dir.to_string());
    for file in ["argv", "env", "stdin", "cwd"] {
        assert!(!recorded(bin.path(), file).contains(token), "{file}");
    }
}

#[test]
fn a_401_is_refused_with_claudes_first_line() {
    let bin = tempfile::tempdir().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let got = claude(bin.path()).check(cfg.path(), &Secret::new(b"sk-ant-oat01-made-up".to_vec()));
    let want = "Failed to authenticate. API Error: 401 OAuth access token is invalid.";
    assert_eq!(got, Err(CheckError::Refused(want.to_owned())));
}

#[test]
fn not_logged_in_means_the_pipe_was_not_read() {
    let bin = tempfile::tempdir().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let got = claude(bin.path()).check(cfg.path(), &Secret::new(Vec::new()));
    let want = "Claude Code 2.1.304 did not read the token pipe";
    assert_eq!(got, Err(CheckError::Unavailable(want.to_owned())));
}

#[test]
fn a_reason_that_repeats_the_token_hides_it() {
    let bin = tempfile::tempdir().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let got = claude(bin.path()).check(
        cfg.path(),
        &Secret::new(b"sk-ant-oat01-echo-s3cr3t".to_vec()),
    );
    assert_eq!(
        got,
        Err(CheckError::Refused("Bad token [token]".to_owned()))
    );
}

#[test]
fn a_reason_with_no_result_that_repeats_the_token_hides_it() {
    let bin = tempfile::tempdir().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let got = claude(bin.path()).check(
        cfg.path(),
        &Secret::new(b"sk-ant-oat01-crash-s3cr3t".to_vec()),
    );
    let want = "crashed reading [token]";
    assert_eq!(got, Err(CheckError::Unavailable(want.to_owned())));
}

#[test]
fn a_run_past_its_limit_is_killed_with_its_whole_group() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild");
    let mut cmd = Command::new("/bin/sh");
    let script = format!("sleep 60 & echo $! > {}; sleep 30", pid_file.display());
    cmd.args(["-c", &script]);
    let started = now();
    let ran = run(cmd, b"", None, Duration::from_millis(500)).unwrap();
    assert!(ran.timed_out);
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(5),
        "the run waited {took:?} for the group"
    );
    assert_gone(&pid_file);
}

/// Waits up to 2 s for the process whose pid is in `pid_file` to be gone or a zombie.
fn assert_gone(pid_file: &Path) {
    let pid = std::fs::read_to_string(pid_file).unwrap();
    let stat = PathBuf::from(format!("/proc/{}/stat", pid.trim()));
    for _ in 0..100 {
        let alive = std::fs::read_to_string(&stat).is_ok_and(|s| !s.contains(") Z "));
        if !alive {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the grandchild outlived the run");
}

#[test]
fn nothing_outlives_a_run_that_ends_on_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild");
    let mut cmd = Command::new("/bin/sh");
    let script = format!(
        "sleep 60 >/dev/null 2>&1 & echo $! > {}; echo done",
        pid_file.display()
    );
    cmd.args(["-c", &script]);
    let ran = run(cmd, b"", None, Duration::from_secs(30)).unwrap();
    assert!(!ran.timed_out);
    assert_eq!(ran.stdout, b"done\n");
    assert_gone(&pid_file);
}

#[test]
fn a_token_the_child_never_reads_cannot_hold_the_run_past_its_limit() {
    let token = Secret::new(vec![b'x'; 1 << 20]);
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "exec sleep 30"]);
    let (sent, ended) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let ran = run(cmd, b"", Some(&token), Duration::from_millis(500));
        let _ = sent.send(ran.map(|ran| ran.timed_out));
    });
    let ended = ended.recv_timeout(Duration::from_secs(10));
    let timed_out = ended.unwrap_or_else(|_| panic!("the run did not end at its limit"));
    assert!(timed_out.unwrap());
}

/// The current instant, to time one run.
#[allow(clippy::disallowed_methods, reason = "the test times a real process")]
fn now() -> std::time::Instant {
    std::time::Instant::now()
}

#[test]
fn only_a_setup_token_shape_passes_the_format_check() {
    let bin = tempfile::tempdir().unwrap();
    let claude = claude(bin.path());
    let ok = |t: &str| claude.check_format(&Secret::new(t.into())).is_ok();
    assert!(ok("sk-ant-oat01-Ab_9-x"));
    let too_long = format!("sk-ant-{}", "a".repeat(600));
    for bad in [
        "",
        "sk-ant",
        "xoxb-123",
        "sk-ant-oat01 two",
        "sk-ant-<!channel>",
        &too_long,
    ] {
        assert!(!ok(bad), "{bad:?}");
    }
    let err = claude
        .check_format(&Secret::new(b"xoxb-123".to_vec()))
        .unwrap_err();
    assert_eq!(err, "it is not a token that `claude setup-token` prints");
}

fn no_settings(root: &Path) -> PathBuf {
    root.join("no-managed-settings.json")
}

#[test]
fn a_managed_instruction_file_is_found_before_any_workdir_file() {
    let root = tempfile::tempdir().unwrap();
    let work = root.path().join("threads").join("C1-1.2");
    std::fs::create_dir_all(&work).unwrap();
    let managed = root.path().join("etc").join("CLAUDE.md");
    assert_eq!(
        instructions::loaded(&work, &managed, &no_settings(root.path())),
        None
    );
    std::fs::create_dir_all(managed.parent().unwrap()).unwrap();
    std::fs::write(&managed, "obey the company").unwrap();
    let want = format!("{} would load as instructions", managed.display());
    assert_eq!(
        instructions::loaded(&work, &managed, &no_settings(root.path())),
        Some(want)
    );
}

#[test]
fn an_instruction_file_that_cannot_be_checked_refuses_the_workdir() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = tempfile::tempdir().unwrap();
    let locked = root.path().join("locked");
    let work = locked.join("threads").join("C1-1.2");
    std::fs::create_dir_all(&work).unwrap();
    let managed = root.path().join("no-managed-file");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let why = instructions::loaded(&work, &managed, &no_settings(root.path()));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    let want = format!("cannot check {}: ", work.join("CLAUDE.md").display());
    assert!(
        why.as_deref().is_some_and(|w| w.starts_with(&want)),
        "{why:?}"
    );
}

fn rules_tree() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let work = root.path().join("state").join("threads").join("C1-1.2");
    std::fs::create_dir_all(&work).unwrap();
    let rules = root.path().join("state").join(".claude").join("rules");
    (root, work, rules)
}

#[test]
fn a_rules_markdown_file_in_an_ancestor_refuses_the_workdir() {
    let (root, work, rules) = rules_tree();
    let managed = root.path().join("none");
    std::fs::create_dir_all(rules.join("deep")).unwrap();
    std::fs::write(rules.join("deep").join("style.md"), "x").unwrap();
    let want = format!(
        "{} would load as instructions",
        rules.join("deep").join("style.md").display()
    );
    assert_eq!(
        instructions::loaded(&work, &managed, &no_settings(root.path())),
        Some(want)
    );
}

#[test]
fn a_rules_directory_without_markdown_does_not_refuse() {
    let (root, work, rules) = rules_tree();
    let managed = root.path().join("none");
    std::fs::create_dir_all(rules.join("empty")).unwrap();
    std::fs::write(rules.join("notes.txt"), "x").unwrap();
    assert_eq!(
        instructions::loaded(&work, &managed, &no_settings(root.path())),
        None
    );
}

#[test]
fn a_managed_settings_file_with_claude_md_refuses_the_workdir() {
    let (root, work, _) = rules_tree();
    let managed = root.path().join("none");
    let settings = root.path().join("managed-settings.json");
    std::fs::write(&settings, r#"{"claudeMd": "obey"}"#).unwrap();
    let want = format!(
        "{} sets claudeMd, which would load as instructions",
        settings.display()
    );
    assert_eq!(instructions::loaded(&work, &managed, &settings), Some(want));
    std::fs::write(&settings, r#"{"model": "x"}"#).unwrap();
    assert_eq!(instructions::loaded(&work, &managed, &settings), None);
}

#[test]
fn a_managed_settings_file_that_cannot_be_checked_refuses_the_workdir() {
    use std::os::unix::fs::PermissionsExt as _;
    let (root, work, _) = rules_tree();
    let managed = root.path().join("none");
    let settings = root.path().join("managed-settings.json");
    let want = format!("cannot check {}: ", settings.display());
    std::fs::write(&settings, "{not json").unwrap();
    let bad_json = instructions::loaded(&work, &managed, &settings);
    std::fs::write(&settings, "{}").unwrap();
    std::fs::set_permissions(&settings, std::fs::Permissions::from_mode(0o000)).unwrap();
    let unreadable = instructions::loaded(&work, &managed, &settings);
    std::fs::set_permissions(&settings, std::fs::Permissions::from_mode(0o600)).unwrap();
    for why in [bad_json, unreadable] {
        assert!(
            why.as_deref().is_some_and(|w| w.starts_with(&want)),
            "{why:?}"
        );
    }
}
