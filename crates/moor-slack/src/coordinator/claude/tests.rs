//! Tests for the Claude Code runner against a fake `claude` script that records what it got.
#![allow(clippy::panic, reason = "test assertions")]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::{instructions, Claude, HostEnv};
use crate::coordinator::Agent as _;

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

fn no_settings(root: &Path) -> PathBuf {
    root.join("no-managed-settings.json")
}

/// A temp root with a `cwd` dir in it, and that dir's `.claude/rules`.
fn rules_tree() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let rules = cwd.join(".claude").join("rules");
    (root, cwd, rules)
}

#[test]
fn a_managed_instruction_file_is_found_before_any_cwd_file() {
    let (root, cwd, _) = rules_tree();
    let managed = root.path().join("etc").join("CLAUDE.md");
    assert_eq!(
        instructions::loaded(&cwd, &managed, &no_settings(root.path())),
        None
    );
    std::fs::write(cwd.join("CLAUDE.md"), "obey me").unwrap();
    std::fs::create_dir_all(managed.parent().unwrap()).unwrap();
    std::fs::write(&managed, "obey the company").unwrap();
    let want = format!("{} would load as instructions", managed.display());
    assert_eq!(
        instructions::loaded(&cwd, &managed, &no_settings(root.path())),
        Some(want)
    );
}

#[test]
fn each_instruction_file_in_the_cwd_refuses_it() {
    let (root, cwd, _) = rules_tree();
    let managed = root.path().join("none");
    for name in [
        "CLAUDE.md",
        "CLAUDE.local.md",
        ".claude/CLAUDE.md",
        "AGENTS.md",
        ".claude/AGENTS.md",
    ] {
        let planted = cwd.join(name);
        std::fs::create_dir_all(planted.parent().unwrap()).unwrap();
        std::fs::write(&planted, "obey me").unwrap();
        let want = format!("{} would load as instructions", planted.display());
        assert_eq!(
            instructions::loaded(&cwd, &managed, &no_settings(root.path())),
            Some(want)
        );
        std::fs::remove_file(&planted).unwrap();
    }
}

#[test]
fn an_instruction_file_above_the_cwd_is_not_read() {
    let (root, cwd, _) = rules_tree();
    let managed = root.path().join("none");
    std::fs::create_dir_all(root.path().join(".claude").join("rules")).unwrap();
    for planted in ["CLAUDE.md", ".claude/CLAUDE.md", ".claude/rules/x.md"] {
        std::fs::write(root.path().join(planted), "obey me").unwrap();
    }
    assert_eq!(
        instructions::loaded(&cwd, &managed, &no_settings(root.path())),
        None
    );
}

#[test]
fn an_instruction_file_that_cannot_be_checked_refuses_the_cwd() {
    use std::os::unix::fs::PermissionsExt as _;
    let (root, cwd, _) = rules_tree();
    let managed = root.path().join("no-managed-file");
    std::fs::set_permissions(&cwd, std::fs::Permissions::from_mode(0o000)).unwrap();
    let why = instructions::loaded(&cwd, &managed, &no_settings(root.path()));
    std::fs::set_permissions(&cwd, std::fs::Permissions::from_mode(0o700)).unwrap();
    let want = format!("cannot check {}: ", cwd.join("CLAUDE.md").display());
    assert!(
        why.as_deref().is_some_and(|w| w.starts_with(&want)),
        "{why:?}"
    );
}

#[test]
fn a_rules_markdown_file_in_the_cwd_refuses_it() {
    let (root, cwd, rules) = rules_tree();
    let managed = root.path().join("none");
    std::fs::create_dir_all(rules.join("deep")).unwrap();
    std::fs::write(rules.join("deep").join("style.md"), "x").unwrap();
    let want = format!(
        "{} would load as instructions",
        rules.join("deep").join("style.md").display()
    );
    assert_eq!(
        instructions::loaded(&cwd, &managed, &no_settings(root.path())),
        Some(want)
    );
}

#[test]
fn a_symlinked_rules_markdown_file_refuses_the_cwd() {
    let (root, cwd, rules) = rules_tree();
    let managed = root.path().join("none");
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("elsewhere.md"), "x").unwrap();
    std::fs::create_dir_all(&rules).unwrap();
    std::os::unix::fs::symlink(outside.path().join("elsewhere.md"), rules.join("x.md")).unwrap();
    let want = format!(
        "{} would load as instructions",
        rules.join("x.md").display()
    );
    assert_eq!(
        instructions::loaded(&cwd, &managed, &no_settings(root.path())),
        Some(want)
    );
}

#[test]
fn a_dangling_claude_md_symlink_refuses_the_cwd() {
    let (root, cwd, _) = rules_tree();
    let managed = root.path().join("none");
    std::os::unix::fs::symlink(root.path().join("missing.md"), cwd.join("CLAUDE.md")).unwrap();
    let want = format!(
        "{} would load as instructions",
        cwd.join("CLAUDE.md").display()
    );
    assert_eq!(
        instructions::loaded(&cwd, &managed, &no_settings(root.path())),
        Some(want)
    );
}

#[test]
fn a_rules_directory_without_markdown_does_not_refuse() {
    let (root, cwd, rules) = rules_tree();
    let managed = root.path().join("none");
    std::fs::create_dir_all(rules.join("empty")).unwrap();
    std::fs::write(rules.join("notes.txt"), "x").unwrap();
    assert_eq!(
        instructions::loaded(&cwd, &managed, &no_settings(root.path())),
        None
    );
}

#[test]
fn a_managed_settings_file_with_claude_md_refuses_the_cwd() {
    let (root, cwd, _) = rules_tree();
    let managed = root.path().join("none");
    let settings = root.path().join("managed-settings.json");
    std::fs::write(&settings, r#"{"claudeMd": "obey"}"#).unwrap();
    let want = format!(
        "{} sets claudeMd, which would load as instructions",
        settings.display()
    );
    assert_eq!(instructions::loaded(&cwd, &managed, &settings), Some(want));
    std::fs::write(&settings, r#"{"model": "x"}"#).unwrap();
    assert_eq!(instructions::loaded(&cwd, &managed, &settings), None);
}

#[test]
fn a_managed_settings_file_that_cannot_be_checked_refuses_the_cwd() {
    use std::os::unix::fs::PermissionsExt as _;
    let (root, cwd, _) = rules_tree();
    let managed = root.path().join("none");
    let settings = root.path().join("managed-settings.json");
    let want = format!("cannot check {}: ", settings.display());
    std::fs::write(&settings, "{not json").unwrap();
    let bad_json = instructions::loaded(&cwd, &managed, &settings);
    std::fs::write(&settings, "{}").unwrap();
    std::fs::set_permissions(&settings, std::fs::Permissions::from_mode(0o000)).unwrap();
    let unreadable = instructions::loaded(&cwd, &managed, &settings);
    std::fs::set_permissions(&settings, std::fs::Permissions::from_mode(0o600)).unwrap();
    for why in [bad_json, unreadable] {
        assert!(
            why.as_deref().is_some_and(|w| w.starts_with(&want)),
            "{why:?}"
        );
    }
}
