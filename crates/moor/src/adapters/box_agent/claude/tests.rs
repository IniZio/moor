use moor_core::activity::Question;
use moor_core::seam::agent_adapter::{AgentAdapter, FirstRunPrompt, Key, NOTHING_TO_ARCHIVE};
use serde_json::json;

use super::Claude;

#[test]
fn escape_interrupts_the_agent() {
    assert_eq!(Claude.interrupt_key(), Key::Escape);
}

#[test]
fn the_note_keeps_the_session_id_and_the_last_reply() {
    let note = Claude.parse_note(&json!({"session_id": "s-1", "said": "Done."}));
    assert_eq!(note.session_id.as_deref(), Some("s-1"));
    assert_eq!(note.said.as_deref(), Some("Done."));
    let empty = Claude.parse_note(&json!({"session_id": 7}));
    assert_eq!(empty.session_id, None);
    assert_eq!(empty.said, None);
}

#[test]
fn the_note_reads_the_first_question_with_its_option_labels() {
    let note = Claude.parse_note(&json!({"questions": [
        {"question": "Which fruit?", "options": [{"label": "Apple"}, {"label": "Banana"}]},
        {"question": "Later?"}
    ]}));
    assert_eq!(
        note.question,
        Some(Question {
            text: "Which fruit?".to_owned(),
            options: vec!["Apple".to_owned(), "Banana".to_owned()],
        })
    );
    assert_eq!(Claude.parse_note(&json!({"questions": []})).question, None);
}

#[test]
fn the_note_lists_background_work_and_whether_all_of_it_is_local() {
    let note = Claude.parse_note(&json!({
        "background": ["gh run watch", 3],
        "background_local": true
    }));
    assert_eq!(note.background, ["gh run watch"]);
    assert!(note.background_local);
    let empty = Claude.parse_note(&json!({"background": [], "background_local": "yes"}));
    assert!(empty.background.is_empty());
    assert!(!empty.background_local);
}

#[test]
fn a_note_that_is_not_an_object_is_empty() {
    assert_eq!(
        Claude.parse_note(&json!(null)),
        moor_core::activity::Note::default()
    );
    assert_eq!(
        Claude.parse_note(&json!([1, 2])),
        moor_core::activity::Note::default()
    );
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
    assert_eq!(
        first_run("New MCP server found in this project: db"),
        Some(FirstRunPrompt::McpServers)
    );
    assert_eq!(
        first_run("3 new MCP servers found in this project"),
        Some(FirstRunPrompt::McpServers)
    );
    assert_eq!(
        first_run("Allow external CLAUDE.md file imports?"),
        Some(FirstRunPrompt::ExternalImports)
    );
}

#[test]
fn every_first_run_prompt_has_a_distinct_name() {
    let names = [
        FirstRunPrompt::BypassPermissions,
        FirstRunPrompt::FolderTrust,
        FirstRunPrompt::PressEnter,
        FirstRunPrompt::SecurityNotes,
        FirstRunPrompt::Theme,
        FirstRunPrompt::McpServers,
        FirstRunPrompt::ExternalImports,
    ]
    .map(FirstRunPrompt::name);
    assert_eq!(names[5..], ["mcp_servers", "external_imports"]);
    let unique: std::collections::BTreeSet<_> = names.iter().collect();
    assert_eq!(unique.len(), names.len());
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
    assert!(script.contains("$HOME/.claude/projects/$(pwd -P"));
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
    moor_testkit::write_executable(&tar, "#!/bin/sh\n: > \"$2\"\n").unwrap();
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

#[test]
fn claudes_folder_trust_screen_is_a_first_run_prompt() {
    let screen = include_str!("fixtures/folder_trust.txt");
    let m = Claude.first_run_prompt(screen).expect("a first-run prompt");
    assert_eq!(m.prompt, FirstRunPrompt::FolderTrust);
    assert_eq!(m.line, "Yes, I trust this folder");
}

#[test]
fn transcript_script_finds_the_sessions_of_a_workdir_reached_through_a_symlink() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let real = root.path().join("real");
    let link = root.path().join("link");
    std::fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let dir = project_dir(&home, &real);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("s.jsonl"), "{}\n").unwrap();
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "cd {} && {}",
            link.display(),
            Claude.transcript_script(".moor/t.tgz")
        ))
        .env("HOME", &home)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_ne!(
        String::from_utf8_lossy(&out.stdout).trim(),
        NOTHING_TO_ARCHIVE
    );
    assert!(real.join(".moor/t.tgz").exists());
}
