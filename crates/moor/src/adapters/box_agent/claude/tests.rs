use moor_core::activity::{HookKind, HookRecord};
use moor_core::seam::agent_profile::{AgentProfile, FirstRunPrompt, NOTHING_TO_ARCHIVE};
use moor_core::seam::clock::Ts;

use super::Claude;

fn parse(raw: &str) -> Option<HookRecord> {
    Claude.parse_hook(raw.as_bytes())
}

fn kind(raw: &str) -> Option<HookKind> {
    parse(raw).map(|r| r.kind)
}

#[test]
fn working_state_is_busy() {
    assert_eq!(
        kind(r#"{"state":"working","event":"UserPromptSubmit","ts":1760000000}"#),
        Some(HookKind::Busy)
    );
}

#[test]
fn idle_state_is_idle() {
    assert_eq!(kind(r#"{"state":"idle","ts":1}"#), Some(HookKind::Idle));
}

#[test]
fn starting_state_is_starting() {
    assert_eq!(
        kind(r#"{"state":"starting","ts":1}"#),
        Some(HookKind::Starting)
    );
}

#[test]
fn waiting_input_carries_the_message() {
    let rec = parse(r#"{"state":"waiting_input","message":"Allow rm?","ts":1}"#).unwrap();
    assert_eq!(rec.kind, HookKind::Blocked);
    assert_eq!(rec.message.as_deref(), Some("Allow rm?"));
}

#[test]
fn waiting_input_without_message_has_none() {
    let rec = parse(r#"{"state":"waiting_input","ts":1}"#).unwrap();
    assert_eq!(rec.kind, HookKind::Blocked);
    assert_eq!(rec.message, None);
}

#[test]
fn session_id_is_kept_when_a_string() {
    let rec = parse(r#"{"state":"idle","session_id":"s-1","ts":1}"#).unwrap();
    assert_eq!(rec.session_id.as_deref(), Some("s-1"));
    let rec = parse(r#"{"state":"idle","ts":1}"#).unwrap();
    assert_eq!(rec.session_id, None);
}

#[test]
fn idle_prompt_notification_is_idle_not_blocked() {
    assert_eq!(
        kind(r#"{"state":"waiting_input","notification_type":"idle_prompt","message":"x","ts":1}"#),
        Some(HookKind::Idle)
    );
}

#[test]
fn timestamp_seconds_become_milliseconds() {
    let rec = parse(r#"{"state":"working","ts":1760000000}"#).unwrap();
    assert_eq!(rec.at, Ts::from_millis(1_760_000_000_000));
}

#[test]
fn invalid_json_is_absent() {
    assert_eq!(parse("not json"), None);
    assert_eq!(Claude.parse_hook(&[0xff, 0xfe]), None);
}

#[test]
fn truncated_json_is_absent() {
    assert_eq!(parse(r#"{"state":"working","ts":17600"#), None);
}

#[test]
fn non_object_json_is_absent() {
    assert_eq!(parse("[1,2]"), None);
}

#[test]
fn missing_state_is_absent() {
    assert_eq!(parse(r#"{"ts":1}"#), None);
}

#[test]
fn unknown_state_is_absent() {
    assert_eq!(parse(r#"{"state":"napping","ts":1}"#), None);
}

#[test]
fn missing_ts_is_absent() {
    assert_eq!(parse(r#"{"state":"idle"}"#), None);
}

#[test]
fn string_ts_is_absent() {
    assert_eq!(parse(r#"{"state":"idle","ts":"1760000000"}"#), None);
}

#[test]
fn negative_or_overflowing_ts_is_absent() {
    assert_eq!(parse(r#"{"state":"idle","ts":-1}"#), None);
    assert_eq!(parse(r#"{"state":"idle","ts":9223372036854775807}"#), None);
}

const RULE: &str = "────────────────";

fn prompt(tail: &str) -> Option<String> {
    Claude.pane_prompt(tail)
}

#[test]
fn pane_prompt_is_the_line_above_the_input_box() {
    let tail = format!("Allow rm?\n\n{RULE}\n> \n{RULE}\n  ? for shortcuts");
    assert_eq!(prompt(&tail).as_deref(), Some("Allow rm?"));
    let boxed = "Allow rm?\n╭──────╮\n│ >    │\n╰──────╯\n  esc to interrupt";
    assert_eq!(prompt(boxed).as_deref(), Some("Allow rm?"));
}

#[test]
fn pane_prompt_skips_blank_lines_and_the_footer() {
    let tail = format!("Allow rm?\n  \n\n{RULE}\n> \n{RULE}\n  esc to interrupt");
    assert_eq!(prompt(&tail).as_deref(), Some("Allow rm?"));
}

#[test]
fn pane_prompt_needs_an_input_box_and_text_above_it() {
    assert_eq!(prompt("esc to interrupt"), None);
    assert_eq!(prompt(&format!("{RULE}\n> \n{RULE}\nfooter")), None);
}

fn first_run(tail: &str) -> Option<FirstRunPrompt> {
    Claude.first_run_prompt(tail).map(|m| m.prompt)
}

#[test]
fn first_run_prompts_are_told_apart() {
    assert_eq!(
        first_run("Do you trust the files in this folder?"),
        Some(FirstRunPrompt::FolderTrust)
    );
    assert_eq!(
        first_run("│ Press Enter to continue │"),
        Some(FirstRunPrompt::PressEnter)
    );
    assert_eq!(
        first_run("Security notes:\n1. It can make mistakes"),
        Some(FirstRunPrompt::SecurityNotes)
    );
    assert_eq!(
        first_run("Choose the text style that looks best with your terminal"),
        Some(FirstRunPrompt::Theme)
    );
    assert_eq!(
        first_run("WARNING: Claude Code running in Bypass Permissions mode"),
        Some(FirstRunPrompt::BypassPermissions)
    );
    assert_eq!(first_run("❯ 2. Light mode"), Some(FirstRunPrompt::Theme));
}

#[test]
fn ordinary_pane_text_is_no_first_run_prompt() {
    assert_eq!(first_run("● Reading the files\n> "), None);
    assert_eq!(first_run("Yes, proceed"), None);
}

#[test]
fn the_bottom_prompt_wins() {
    let tail = "Security notes:\nPress Enter to continue\n";
    assert_eq!(first_run(tail), Some(FirstRunPrompt::PressEnter));
    let tail = "Press Enter to continue\nSecurity notes:\n";
    assert_eq!(first_run(tail), Some(FirstRunPrompt::SecurityNotes));
}

#[test]
fn yes_proceed_is_folder_trust_only_when_the_pane_mentions_trust() {
    let tail = "Do you want to trust it?\n 1. Yes, proceed\n";
    assert_eq!(first_run(tail), Some(FirstRunPrompt::FolderTrust));
}

#[test]
fn the_matched_line_loses_box_edges_and_ansi_codes() {
    let tail = "\x1b[1m\x1b[38;5;214m│ Press Enter to continue \x1b[0m│";
    let m = Claude.first_run_prompt(tail).unwrap();
    assert_eq!(m.prompt, FirstRunPrompt::PressEnter);
    assert_eq!(m.line, "Press Enter to continue");
    let m = Claude
        .first_run_prompt("╭──╮\n│ Security notes │\n╰──╯")
        .unwrap();
    assert_eq!(m.line, "Security notes");
}

#[test]
fn operating_system_commands_are_stripped_from_the_matched_line() {
    let bell = "\x1b]0;title\x07Press Enter to continue";
    assert_eq!(
        Claude.first_run_prompt(bell).unwrap().line,
        "Press Enter to continue"
    );
    let st = "\x1b]8;;http://x\x1b\\Press Enter to continue";
    assert_eq!(
        Claude.first_run_prompt(st).unwrap().line,
        "Press Enter to continue"
    );
}

#[test]
fn transcript_script_archives_the_project_sessions_to_the_given_path() {
    let script = Claude.transcript_script(".moor/t.tgz");
    assert!(script.contains("$HOME/.claude/projects/$enc"));
    assert!(script.contains("*.jsonl"));
    assert!(script.contains("tar czf \"$OLDPWD/.moor/t.tgz\""));
    assert!(script.contains(NOTHING_TO_ARCHIVE));
}

/// Runs the transcript script in a fresh workdir under a fresh HOME; `setup` gets (home, workdir).
fn run_script(
    setup: impl FnOnce(&std::path::Path, &std::path::Path),
) -> (std::process::Output, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let work = root.path().join("work");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    setup(&home, &work);
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(Claude.transcript_script(".moor/t.tgz"))
        .env("HOME", &home)
        .current_dir(&work)
        .output()
        .unwrap();
    (out, root)
}

/// The project directory the script reads for `work`.
fn project_dir(home: &std::path::Path, work: &std::path::Path) -> std::path::PathBuf {
    let enc: String = std::fs::canonicalize(work)
        .unwrap()
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    home.join(".claude/projects").join(enc)
}

#[test]
fn transcript_script_prints_the_marker_and_exits_zero_without_a_jsonl() {
    for with_dir in [false, true] {
        let (out, _root) = run_script(|home, work| {
            if with_dir {
                std::fs::create_dir_all(project_dir(home, work)).unwrap();
            }
        });
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            NOTHING_TO_ARCHIVE
        );
    }
}

#[test]
fn transcript_script_fails_when_the_archive_is_empty() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let work = root.path().join("work");
    let bin = root.path().join("bin");
    for d in [&home, &work, &bin] {
        std::fs::create_dir_all(d).unwrap();
    }
    let tar = bin.join("tar");
    std::fs::write(&tar, "#!/bin/sh\n: > \"$2\"\n").unwrap();
    std::fs::set_permissions(&tar, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let d = project_dir(&home, &work);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("a.jsonl"), "{}").unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(Claude.transcript_script(".moor/t.tgz"))
        .env("HOME", &home)
        .env("PATH", path)
        .current_dir(&work)
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0), "{out:?}");
}

#[test]
fn transcript_script_archives_when_a_jsonl_exists() {
    let (out, _root) = run_script(|home, work| {
        let d = project_dir(home, work);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.jsonl"), "{}").unwrap();
    });
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains(NOTHING_TO_ARCHIVE));
}

#[test]
fn transcript_script_fails_when_tar_fails() {
    use std::os::unix::fs::PermissionsExt;
    let mut file = None;
    let (out, _root) = run_script(|home, work| {
        let d = project_dir(home, work);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("a.jsonl");
        std::fs::write(&f, "{}").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).unwrap();
        file = Some(f);
    });
    let readable_anyway = std::fs::read(file.unwrap()).is_ok();
    if !readable_anyway {
        assert_ne!(out.status.code(), Some(0), "{out:?}");
    }
}

#[test]
fn transcript_script_fails_when_the_project_dir_is_unreadable() {
    use std::os::unix::fs::PermissionsExt;
    let mut dir = None;
    let (out, _root) = run_script(|home, work| {
        let d = project_dir(home, work);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.jsonl"), "{}").unwrap();
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o000)).unwrap();
        dir = Some(d);
    });
    let d = dir.unwrap();
    let readable_anyway = std::fs::read_dir(&d).is_ok();
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
    if !readable_anyway {
        assert_ne!(out.status.code(), Some(0), "{out:?}");
    }
}
