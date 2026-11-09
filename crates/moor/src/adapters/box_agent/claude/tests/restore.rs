//! An archive from one machine puts the conversation back on another, whose work tree may sit
//! at a different path.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use moor_core::seam::agent_adapter::AgentAdapter;

use super::project_dir;
use crate::adapters::box_agent::claude::Claude;

const ARCHIVE: &str = ".moor/restore.tgz";

struct Machine {
    _root: tempfile::TempDir,
    home: PathBuf,
    work: PathBuf,
}

fn machine(work: &str) -> Machine {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let work = root.path().join(work);
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&work).unwrap();
    Machine {
        _root: root,
        home,
        work,
    }
}

fn sh(m: &Machine, script: &str) -> Output {
    Command::new("sh")
        .arg("-c")
        .arg(script)
        .env("HOME", &m.home)
        .current_dir(&m.work)
        .output()
        .unwrap()
}

/// The archive the transcript script makes of `m`'s project directory.
fn archived(m: &Machine) -> Vec<u8> {
    let out = sh(m, &Claude.transcript_script(".moor/t.tgz"));
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    fs::read(m.work.join(".moor/t.tgz")).unwrap()
}

fn restore(m: &Machine, archive: &[u8], session: &str) -> Output {
    fs::create_dir_all(m.work.join(".moor")).unwrap();
    fs::write(m.work.join(ARCHIVE), archive).unwrap();
    sh(m, &Claude.restore_script(session, ARCHIVE))
}

fn holds(m: &Machine, session: &str) -> Option<i32> {
    sh(m, &Claude.session_check_script(session)).status.code()
}

fn old_machine() -> (Machine, Vec<u8>) {
    let old = machine("old-work");
    let d = project_dir(&old.home, &old.work);
    fs::create_dir_all(d.join("s-1/subagents")).unwrap();
    fs::write(d.join("s-1.jsonl"), "{\"said\":\"PLUM-42\"}\n").unwrap();
    fs::write(d.join("s-1/subagents/agent-a.jsonl"), "{}\n").unwrap();
    let archive = archived(&old);
    (old, archive)
}

fn read(d: &Path, name: &str) -> String {
    fs::read_to_string(d.join(name)).unwrap()
}

#[test]
fn a_restore_puts_the_conversation_under_the_new_work_tree() {
    let (_old, archive) = old_machine();
    let new = machine("new-work");
    assert_eq!(holds(&new, "s-1"), Some(1));

    let out = restore(&new, &archive, "s-1");

    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(holds(&new, "s-1"), Some(0));
    let d = project_dir(&new.home, &new.work);
    assert_eq!(read(&d, "s-1.jsonl"), "{\"said\":\"PLUM-42\"}\n");
    assert_eq!(read(&d, "s-1/subagents/agent-a.jsonl"), "{}\n");
    assert!(!new.work.join(ARCHIVE).exists(), "the archive is removed");
    let beside: Vec<_> = fs::read_dir(d.parent().unwrap()).unwrap().collect();
    assert_eq!(beside.len(), 1, "no temp dir is left: {beside:?}");
}

#[test]
fn a_second_restore_keeps_what_the_first_put_back_and_the_agent_wrote_since() {
    let (_old, archive) = old_machine();
    let new = machine("new-work");
    restore(&new, &archive, "s-1");
    let d = project_dir(&new.home, &new.work);
    fs::write(d.join("s-1.jsonl"), "{\"said\":\"TEAL-7\"}\n").unwrap();

    let out = restore(&new, &archive, "s-1");

    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(read(&d, "s-1.jsonl"), "{\"said\":\"TEAL-7\"}\n");
}

#[test]
fn a_restore_keeps_files_the_new_machine_already_has() {
    let (_old, archive) = old_machine();
    let new = machine("new-work");
    let d = project_dir(&new.home, &new.work);
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("s-9.jsonl"), "mine\n").unwrap();

    let out = restore(&new, &archive, "s-1");

    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(read(&d, "s-9.jsonl"), "mine\n");
    assert_eq!(read(&d, "s-1.jsonl"), "{\"said\":\"PLUM-42\"}\n");
}

#[test]
fn an_archive_without_the_session_fails_and_restores_nothing() {
    let (_old, archive) = old_machine();
    let new = machine("new-work");

    let out = restore(&new, &archive, "s-2");

    assert_ne!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(holds(&new, "s-2"), Some(1));
    let d = project_dir(&new.home, &new.work);
    assert!(!d.join("s-1.jsonl").exists());
}

#[test]
fn a_broken_archive_fails() {
    let new = machine("new-work");

    let out = restore(&new, b"not a tar", "s-1");

    assert_ne!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(holds(&new, "s-1"), Some(1));
}
