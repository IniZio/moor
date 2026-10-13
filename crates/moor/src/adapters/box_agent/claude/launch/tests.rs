#![allow(clippy::indexing_slicing, reason = "test assertions")]
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

use moor_core::activity::HookKind;
use moor_core::name::BoxPath;
use moor_core::seam::agent_profile::{AgentProfile, BoxSetup, Key, LaunchCtx};
use serde_json::{json, Value};
use tempfile::TempDir;

use super::super::Claude;

fn ctx(dir: &Path) -> LaunchCtx {
    let root = dir.to_str().unwrap();
    LaunchCtx {
        model: None,
        workdir: BoxPath::under(root, "work").unwrap(),
        bypass: false,
        home: BoxPath::under(root, "home").unwrap(),
    }
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

fn install(setup: &BoxSetup) {
    for file in &setup.files {
        let path = Path::new(file.path.as_str());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, &file.content).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(file.mode)).unwrap();
    }
}

fn run_step(setup: &BoxSetup) -> std::process::Output {
    Command::new("sh")
        .args(["-c", &setup.step])
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn read_json(path: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn argv_is_the_brief_prompt_alone_by_default() {
    let t = TempDir::new().unwrap();
    assert_eq!(
        Claude.argv(&ctx(t.path())),
        strings(&["Read and follow .moor/brief.md"])
    );
}

#[test]
fn argv_names_the_model_before_the_prompt() {
    let t = TempDir::new().unwrap();
    let c = LaunchCtx {
        model: Some("opus".into()),
        ..ctx(t.path())
    };
    assert_eq!(
        Claude.argv(&c),
        strings(&["--model", "opus", "Read and follow .moor/brief.md"])
    );
}

#[test]
fn argv_asks_for_bypass_only_when_allowed() {
    let t = TempDir::new().unwrap();
    let c = LaunchCtx {
        bypass: true,
        ..ctx(t.path())
    };
    assert_eq!(
        Claude.argv(&c),
        strings(&[
            "--permission-mode",
            "bypassPermissions",
            "Read and follow .moor/brief.md"
        ])
    );
    let argv = Claude.argv(&ctx(t.path())).join(" ");
    assert!(!argv.contains("permission") && !argv.contains("--settings"));
}

#[test]
fn resume_argv_keeps_the_flags_and_drops_the_prompt() {
    let t = TempDir::new().unwrap();
    let c = LaunchCtx {
        model: Some("opus".into()),
        bypass: true,
        ..ctx(t.path())
    };
    assert_eq!(
        Claude.resume_argv("s-1", &c),
        strings(&[
            "--resume",
            "s-1",
            "--model",
            "opus",
            "--permission-mode",
            "bypassPermissions"
        ])
    );
}

#[test]
fn menu_keys_are_the_digits() {
    assert_eq!(Claude.menu_keys(2), vec![Key::Char('2')]);
    assert_eq!(Claude.menu_keys(12), vec![Key::Char('1'), Key::Char('2')]);
}

#[test]
fn hook_fragment_covers_the_five_events() {
    let t = TempDir::new().unwrap();
    let setup = Claude.hook_writer(&ctx(t.path())).unwrap();
    let fragment: Value = serde_json::from_str(&setup.files[1].content).unwrap();
    let hooks = &fragment["hooks"];
    let command = |ev: &str| {
        hooks[ev][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert!(command("UserPromptSubmit").ends_with(" working UserPromptSubmit"));
    assert!(command("Stop").ends_with(" idle Stop"));
    assert!(command("PostToolUse").ends_with(" working PostToolUse"));
    assert!(command("Notification").ends_with(" waiting_input Notification"));
    assert!(command("PreToolUse").ends_with(" waiting_input PreToolUse"));
    assert_eq!(
        hooks["Notification"][0]["matcher"],
        "permission_prompt|elicitation_dialog|elicitation_url_dialog|agent_needs_input"
    );
    assert_eq!(
        hooks["PreToolUse"][0]["matcher"],
        "AskUserQuestion|ExitPlanMode"
    );
    assert!(hooks["Stop"][0].get("matcher").is_none());
}

#[test]
fn merge_keeps_existing_settings_and_is_idempotent() {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    let settings = format!("{}/.claude/settings.json", c.home.as_str());
    fs::create_dir_all(Path::new(&settings).parent().unwrap()).unwrap();
    let other = json!({"hooks": [{"type": "command", "command": "echo mine"}]});
    fs::write(
        &settings,
        json!({"model": "x", "hooks": {"Stop": [&other], "SessionStart": [&other]}}).to_string(),
    )
    .unwrap();
    let setup = Claude.hook_writer(&c).unwrap();
    install(&setup);
    let first = run_step(&setup);
    assert!(first.status.success(), "{first:?}");
    let once = read_json(&settings);
    assert_eq!(once["model"], "x");
    assert_eq!(once["hooks"]["SessionStart"], json!([&other]));
    assert_eq!(once["hooks"]["Stop"].as_array().unwrap().len(), 2);
    assert_eq!(once["hooks"]["Stop"][0], other);
    assert_eq!(once["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
    assert!(run_step(&setup).status.success());
    assert_eq!(read_json(&settings), once);
}

#[test]
fn merge_creates_settings_when_absent() {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    let setup = Claude.hook_writer(&c).unwrap();
    install(&setup);
    assert!(run_step(&setup).status.success());
    let settings = read_json(&format!("{}/.claude/settings.json", c.home.as_str()));
    assert_eq!(settings["hooks"].as_object().unwrap().len(), 5);
}

#[test]
fn merge_fails_and_keeps_settings_that_are_not_json() {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    let settings = format!("{}/.claude/settings.json", c.home.as_str());
    fs::create_dir_all(Path::new(&settings).parent().unwrap()).unwrap();
    fs::write(&settings, "not json").unwrap();
    let setup = Claude.hook_writer(&c).unwrap();
    install(&setup);
    assert!(!run_step(&setup).status.success());
    assert_eq!(fs::read_to_string(&settings).unwrap(), "not json");
}

fn run_hook(c: &LaunchCtx, state: &str, event: &str, stdin: Option<&str>) {
    let script = format!("{}/.moor/hook.sh", c.workdir.as_str());
    let mut cmd = Command::new("sh");
    cmd.args([&script, state, event]).stdout(Stdio::null());
    match stdin {
        Some(text) => {
            let mut child = cmd.stdin(Stdio::piped()).spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
            assert!(child.wait().unwrap().success());
        }
        None => assert!(cmd.stdin(Stdio::null()).status().unwrap().success()),
    }
}

fn state_file(c: &LaunchCtx) -> Vec<u8> {
    fs::read(format!("{}/.moor/agent-state.json", c.workdir.as_str())).unwrap()
}

#[test]
fn hook_state_round_trips_through_parse_hook() {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    let payload = json!({
        "session_id": "s-42",
        "message": "Allow \"rm\"?\nreally",
        "notification_type": "permission_prompt",
    })
    .to_string();
    run_hook(&c, "waiting_input", "Notification", Some(&payload));
    let raw = state_file(&c);
    let value: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(value["event"], "Notification");
    assert_eq!(value["notification_type"], "permission_prompt");
    let rec = Claude.parse_hook(&raw).unwrap();
    assert_eq!(rec.session_id.as_deref(), Some("s-42"));
    assert_eq!(rec.kind, HookKind::Blocked);
    assert_eq!(rec.message.as_deref(), Some("Allow \"rm\"?\nreally"));
    let leftovers: Vec<_> = fs::read_dir(format!("{}/.moor", c.workdir.as_str()))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.contains(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn hook_with_closed_stdin_still_records_state() {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    run_hook(&c, "idle", "Stop", None);
    let rec = Claude.parse_hook(&state_file(&c)).unwrap();
    assert_eq!(rec.kind, HookKind::Idle);
    assert_eq!(rec.session_id, None);
}

#[test]
fn hook_with_garbage_stdin_still_records_state() {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    run_hook(&c, "working", "PostToolUse", Some("{not json"));
    let rec = Claude.parse_hook(&state_file(&c)).unwrap();
    assert_eq!(rec.kind, HookKind::Busy);
}

#[test]
fn hook_does_not_wait_for_a_stdin_that_never_closes() {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    let script = format!("{}/.moor/hook.sh", c.workdir.as_str());
    let mut child = Command::new("sh")
        .args([&script, "working", "UserPromptSubmit"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let held = child.stdin.take();
    assert!(child.wait().unwrap().success());
    drop(held);
    assert!(Claude.parse_hook(&state_file(&c)).is_some());
}

#[test]
fn preseed_creates_and_merges_claude_json() {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    let path = format!("{}/.claude.json", c.home.as_str());
    let setup = Claude.preseed(&c).unwrap();
    fs::create_dir_all(c.home.as_str()).unwrap();
    assert!(run_step(&setup).status.success());
    let wd = c.workdir.as_str();
    let fresh = read_json(&path);
    assert_eq!(fresh["hasCompletedOnboarding"], true);
    assert_eq!(fresh["theme"], "dark");
    assert_eq!(fresh["projects"][wd]["hasTrustDialogAccepted"], true);

    fs::write(
        &path,
        json!({"userID": "u", "projects": {wd: {"allowedTools": ["x"]}, "/o": {}}}).to_string(),
    )
    .unwrap();
    assert!(run_step(&setup).status.success());
    let merged = read_json(&path);
    assert_eq!(merged["userID"], "u");
    assert_eq!(merged["projects"]["/o"], json!({}));
    assert_eq!(merged["projects"][wd]["allowedTools"], json!(["x"]));
    assert_eq!(merged["projects"][wd]["hasTrustDialogAccepted"], true);
}

#[test]
fn preseed_failure_warns_and_succeeds() {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    let path = format!("{}/.claude.json", c.home.as_str());
    fs::create_dir_all(c.home.as_str()).unwrap();
    fs::write(&path, "not json").unwrap();
    let out = run_step(&Claude.preseed(&c).unwrap());
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("warning"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "not json");
}
