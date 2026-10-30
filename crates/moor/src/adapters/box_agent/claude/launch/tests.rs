#![allow(
    clippy::indexing_slicing,
    clippy::print_stderr,
    reason = "test assertions and skip notices"
)]
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

use moor_core::activity::HookKind;
use moor_core::name::BoxPath;
use moor_core::seam::agent_adapter::{AgentAdapter, BoxSetup, Key, LaunchCtx, ProxyAddr};
use moor_core::seam::engine::LaunchPrompt;
use serde_json::{json, Value};
use tempfile::TempDir;

use super::super::Claude;

mod ask;
mod bypass;
mod doing;
mod first_run;
mod last_message;
mod mcp;
mod preseed;
mod prompt;

fn ctx(dir: &Path) -> LaunchCtx {
    let root = dir.to_str().unwrap();
    LaunchCtx {
        mcp: std::collections::BTreeMap::new(),
        model: None,
        workdir: BoxPath::under(root, "work").unwrap(),
        proxy: None,
        home: BoxPath::under(root, "home").unwrap(),
        prompt: LaunchPrompt::None,
        instructions: BoxPath::under("/w", ".moor/brief.md").unwrap(),
    }
}

const BRIEF: [&str; 2] = ["--append-system-prompt-file", "/w/.moor/brief.md"];

fn with_brief(rest: &[&str]) -> Vec<String> {
    strings(&[&BRIEF[..], rest].concat())
}

fn proxied(dir: &Path) -> LaunchCtx {
    LaunchCtx {
        mcp: std::collections::BTreeMap::new(),
        proxy: Some(ProxyAddr {
            box_port: 41234,
            ca_pem: String::new(),
            box_env: Vec::new(),
            forge: false,
            egress_port: 41235,
            hosts: Vec::new(),
        }),
        ..ctx(dir)
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
    run_step_on(setup, None)
}

/// Runs the step with `PATH` replaced when given.
fn run_step_on(setup: &BoxSetup, path: Option<&str>) -> std::process::Output {
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", &setup.step]).stdin(Stdio::null());
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    cmd.output().unwrap()
}

/// Which interpreter a forced `PATH` offers.
#[derive(Clone, Copy)]
enum Interp {
    Python,
    Node,
    Neither,
}

impl Interp {
    fn tool(self) -> Option<&'static str> {
        match self {
            Self::Python => Some("python3"),
            Self::Node => Some("node"),
            Self::Neither => None,
        }
    }
}

fn on_host_path(tool: &str) -> Option<String> {
    let out = Command::new("/bin/sh")
        .args(["-c", &format!("command -v {tool}")])
        .output()
        .unwrap();
    out.status
        .success()
        .then(|| String::from_utf8(out.stdout).unwrap().trim().to_owned())
}

/// A `PATH` of symlinks to the shell tools the steps use, plus only the wanted interpreter.
/// `None`, after saying why, when the host lacks a needed tool.
fn forced_path(interp: Interp, bin: &Path) -> Option<String> {
    let wanted = ["head", "timeout", "date", "mv", "rm", "mkdir", "cp", "grep"]
        .into_iter()
        .chain(interp.tool());
    for tool in wanted {
        let Some(real) = on_host_path(tool) else {
            eprintln!("skipped: {tool} is not on the host PATH");
            return None;
        };
        std::os::unix::fs::symlink(real, bin.join(tool)).unwrap();
    }
    Some(bin.to_str().unwrap().to_owned())
}

fn any_interpreter() -> bool {
    let found = ["python3", "node"]
        .iter()
        .any(|t| on_host_path(t).is_some());
    if !found {
        eprintln!("skipped: neither python3 nor node is on the host PATH");
    }
    found
}

fn read_json(path: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn argv_is_the_brief_flag_alone_by_default() {
    let t = TempDir::new().unwrap();
    assert_eq!(Claude.argv(&ctx(t.path())), with_brief(&[]));
}

#[test]
fn argv_names_the_model_before_the_prompt() {
    let t = TempDir::new().unwrap();
    let c = LaunchCtx {
        mcp: std::collections::BTreeMap::new(),
        model: Some("opus".into()),
        prompt: LaunchPrompt::Text("ship it".into()),
        ..ctx(t.path())
    };
    assert_eq!(
        Claude.argv(&c),
        with_brief(&["--model", "opus", "--", "ship it"])
    );
}

#[test]
fn argv_asks_for_bypass_only_behind_a_proxy() {
    let t = TempDir::new().unwrap();
    assert_eq!(
        Claude.argv(&proxied(t.path())),
        with_brief(&["--permission-mode", "bypassPermissions"])
    );
    let argv = Claude.argv(&ctx(t.path())).join(" ");
    assert!(!argv.contains("permission") && !argv.contains("--settings"));
}

#[test]
fn env_is_only_the_login_placeholder_behind_a_proxy() {
    let t = TempDir::new().unwrap();
    let env = Claude.env(&proxied(t.path()));
    let pairs: Vec<(&str, &[u8])> = env
        .iter()
        .map(|e| (e.name.as_str(), e.value.expose()))
        .collect();
    assert_eq!(
        pairs,
        vec![("CLAUDE_CODE_OAUTH_TOKEN", b"moor-proxy".as_slice())]
    );
    assert!(env.iter().all(|e| e.name != "ANTHROPIC_BASE_URL"));
}

#[test]
fn env_is_empty_without_a_proxy() {
    let t = TempDir::new().unwrap();
    assert!(Claude.env(&ctx(t.path())).is_empty());
}

#[test]
fn resume_argv_keeps_the_brief_and_flags_and_drops_the_prompt() {
    let t = TempDir::new().unwrap();
    let c = LaunchCtx {
        mcp: std::collections::BTreeMap::new(),
        model: Some("opus".into()),
        ..proxied(t.path())
    };
    assert_eq!(
        Claude.resume_argv("s-1", &c),
        strings(&[
            "--resume",
            "s-1",
            "--append-system-prompt-file",
            "/w/.moor/brief.md",
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
fn hook_fragment_covers_the_hook_events() {
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
fn a_resumed_session_is_recorded_idle() {
    let t = TempDir::new().unwrap();
    let setup = Claude.hook_writer(&ctx(t.path())).unwrap();
    let fragment: Value = serde_json::from_str(&setup.files[1].content).unwrap();
    let entry = &fragment["hooks"]["SessionStart"][0];
    assert!(entry["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .ends_with(" idle SessionStart"));
    assert_eq!(entry["matcher"], "resume");
}

fn check_merge_keeps_and_is_idempotent(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    let settings = format!("{}/.claude/settings.json", c.home.as_str());
    fs::create_dir_all(Path::new(&settings).parent().unwrap()).unwrap();
    let other = json!({"hooks": [{"type": "command", "command": "echo mine"}]});
    fs::write(
        &settings,
        json!({"model": "x", "hooks": {"Stop": [&other], "PreCompact": [&other]}}).to_string(),
    )
    .unwrap();
    let setup = Claude.hook_writer(&c).unwrap();
    install(&setup);
    let first = run_step_on(&setup, path);
    assert!(first.status.success(), "{first:?}");
    let once = read_json(&settings);
    assert_eq!(once["model"], "x");
    assert_eq!(once["hooks"]["PreCompact"], json!([&other]));
    assert_eq!(once["hooks"]["Stop"].as_array().unwrap().len(), 2);
    assert_eq!(once["hooks"]["Stop"][0], other);
    assert_eq!(once["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
    assert!(run_step_on(&setup, path).status.success());
    assert_eq!(read_json(&settings), once);
}

#[test]
fn merge_keeps_existing_settings_and_is_idempotent() {
    if any_interpreter() {
        check_merge_keeps_and_is_idempotent(None);
    }
}

fn forced(interp: Interp, check: fn(Option<&str>)) {
    let bin = TempDir::new().unwrap();
    if let Some(path) = forced_path(interp, bin.path()) {
        check(Some(&path));
    }
}

#[test]
fn merge_keeps_existing_settings_with_python3_only() {
    forced(Interp::Python, check_merge_keeps_and_is_idempotent);
}

#[test]
fn merge_keeps_existing_settings_with_node_only() {
    forced(Interp::Node, check_merge_keeps_and_is_idempotent);
}

fn check_merge_without_interpreter(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    let settings = format!("{}/.claude/settings.json", c.home.as_str());
    let setup = Claude.hook_writer(&c).unwrap();
    install(&setup);
    assert!(run_step_on(&setup, path).status.success());
    assert_eq!(read_json(&settings)["hooks"].as_object().unwrap().len(), 6);
    fs::write(&settings, r#"{"model":"x"}"#).unwrap();
    assert!(!run_step_on(&setup, path).status.success());
    assert_eq!(fs::read_to_string(&settings).unwrap(), r#"{"model":"x"}"#);
}

#[test]
fn merge_without_an_interpreter_creates_settings_but_never_edits_them() {
    forced(Interp::Neither, check_merge_without_interpreter);
}

#[test]
fn merge_creates_settings_when_absent() {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    let setup = Claude.hook_writer(&c).unwrap();
    install(&setup);
    assert!(run_step(&setup).status.success());
    let settings = read_json(&format!("{}/.claude/settings.json", c.home.as_str()));
    assert_eq!(settings["hooks"].as_object().unwrap().len(), 6);
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
    run_hook_on(c, state, event, stdin, None);
}

fn run_hook_on(c: &LaunchCtx, state: &str, event: &str, stdin: Option<&str>, path: Option<&str>) {
    let script = format!("{}/.moor/hook.sh", c.workdir.as_str());
    let mut cmd = Command::new("/bin/sh");
    cmd.args([&script, state, event]).stdout(Stdio::null());
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
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

fn check_hook_round_trip(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    let payload = json!({
        "session_id": "s-42",
        "message": "Allow \"rm\"?\nreally",
        "notification_type": "permission_prompt",
    })
    .to_string();
    run_hook_on(&c, "waiting_input", "Notification", Some(&payload), path);
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
fn hook_state_round_trips_through_parse_hook() {
    if any_interpreter() {
        check_hook_round_trip(None);
    }
}

#[test]
fn hook_state_round_trips_with_python3_only() {
    forced(Interp::Python, check_hook_round_trip);
}

#[test]
fn hook_state_round_trips_with_node_only() {
    forced(Interp::Node, check_hook_round_trip);
}

fn check_hook_without_interpreter(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    let payload = json!({"session_id": "s-42", "message": "m"}).to_string();
    run_hook_on(&c, "waiting_input", "Notification", Some(&payload), path);
    let raw = state_file(&c);
    let value: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(value["state"], "waiting_input");
    assert_eq!(value["event"], "Notification");
    assert!(value["ts"].as_i64().unwrap() > 0);
    assert_eq!(value["turn"], Value::Null, "no prompt was submitted yet");
    assert_eq!(value.as_object().unwrap().len(), 4);
    let rec = Claude.parse_hook(&raw).unwrap();
    assert_eq!(rec.session_id, None);
}

#[test]
fn hook_without_an_interpreter_falls_back_to_state_event_and_ts() {
    forced(Interp::Neither, check_hook_without_interpreter);
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
