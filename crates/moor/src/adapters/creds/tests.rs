//! Credential source tests: scripted cmd replies, real files and env, one real shell.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::config::CredentialSource;
use moor_core::seam::proc::Tool;

use super::{describe, Creds};
use crate::adapters::proc::{SystemProc, ToolPaths};
use crate::env::Env;
use crate::testing::proc::{Reply, ScriptedProc};

fn creds(env: Env, p: Arc<ScriptedProc>) -> Creds {
    Creds::new(env, p)
}

fn cmd(argv: &[&str]) -> CredentialSource {
    CredentialSource {
        cmd: Some(argv.iter().map(|s| (*s).to_owned()).collect()),
        ..Default::default()
    }
}

fn file(p: &Path) -> CredentialSource {
    CredentialSource {
        file: Some(p.to_path_buf()),
        ..Default::default()
    }
}

fn var(v: &str) -> CredentialSource {
    CredentialSource {
        env: Some(v.to_owned()),
        ..Default::default()
    }
}

fn real() -> Creds {
    let env = Env::capture();
    let proc = Arc::new(SystemProc::new(&env, &ToolPaths::default()));
    Creds::new(env, proc)
}

/// argv prefix `sh` receives for a cmd source.
fn exec(argv: &[&'static str]) -> Vec<&'static str> {
    let mut v = vec!["-c", "exec \"$@\"", "moor-cred"];
    v.extend_from_slice(argv);
    v
}

#[test]
fn cmd_ok_fail_blank() {
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Sh, &exec(&["ok"]), Reply::Ok("x\n".into()))
        .on(
            Tool::Sh,
            &exec(&["bad"]),
            Reply::Fail {
                code: 1,
                stdout: "s3cr3t".into(),
                stderr: String::new(),
            },
        )
        .on(Tool::Sh, &exec(&["blank"]), Reply::Ok("  \n".into()))
        .on(Tool::Sh, &exec(&["slow"]), Reply::Timeout)
        .on(
            Tool::Sh,
            &exec(&["bin"]),
            Reply::Bytes(vec![0xff, 0xfe, b'x']),
        );
    let c = creds(Env::default(), p);
    assert_eq!(c.check(&cmd(&["ok"])), Ok(()));
    let err = c.check(&cmd(&["bad"])).unwrap_err();
    assert_eq!(err, "cmd exited 1");
    assert!(!err.contains("s3cr3t"));
    assert_eq!(
        c.check(&cmd(&["blank"])),
        Err("cmd printed nothing".to_owned())
    );
    assert!(c.check(&cmd(&["slow"])).unwrap_err().contains("timed out"));
    assert_eq!(c.check(&cmd(&["bin"])), Ok(()));
}

#[test]
fn argv_cmd_runs_through_exec() {
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Sh, &exec(&["op", "read"]), Reply::Ok("v".into()));
    let c = creds(Env::default(), p);
    assert_eq!(c.check(&cmd(&["op", "read"])), Ok(()));
}

#[test]
fn file_present_missing_empty_dir() {
    let d = tempfile::TempDir::new().unwrap();
    let ok = d.path().join("ok");
    std::fs::write(&ok, "tok\n").unwrap();
    let empty = d.path().join("empty");
    std::fs::write(&empty, "").unwrap();
    let blank = d.path().join("blank");
    std::fs::write(&blank, "  \n").unwrap();
    let c = real();
    assert_eq!(c.check(&file(&ok)), Ok(()));
    assert_eq!(c.check(&file(&blank)), Err("file is empty".to_owned()));
    assert_eq!(
        c.check(&file(&d.path().join("nope"))),
        Err("not found".to_owned())
    );
    assert_eq!(c.check(&file(&empty)), Err("file is empty".to_owned()));
    assert_eq!(
        c.check(&file(d.path())),
        Err("not a regular file".to_owned())
    );
}

#[test]
fn file_tilde_expands_home() {
    let d = tempfile::TempDir::new().unwrap();
    std::fs::write(d.path().join("t"), "v").unwrap();
    let env = Env::from_pairs([("HOME", d.path().to_str().unwrap())]);
    let c = creds(env, Arc::new(ScriptedProc::new()));
    assert_eq!(c.check(&file(&PathBuf::from("~/t"))), Ok(()));
    assert_eq!(
        c.check(&file(&PathBuf::from("~/missing"))),
        Err("not found".to_owned())
    );
}

#[test]
fn env_set_unset_empty() {
    let env = Env::from_pairs([("SET", "v"), ("EMPTY", ""), ("BLANK", " \t")]);
    let c = creds(env, Arc::new(ScriptedProc::new()));
    assert_eq!(c.check(&var("SET")), Ok(()));
    assert_eq!(c.check(&var("EMPTY")), Err("env EMPTY is empty".to_owned()));
    assert_eq!(c.check(&var("BLANK")), Err("env BLANK is empty".to_owned()));
    assert_eq!(c.check(&var("NOPE")), Err("env NOPE is not set".to_owned()));
}

#[test]
fn whitespace_only_cmd_file_env_are_blank() {
    let d = tempfile::TempDir::new().unwrap();
    let f = d.path().join("ws");
    std::fs::write(&f, " \r\n\t").unwrap();
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Sh, &exec(&["ws"]), Reply::Ok(" \r\n\t".into()));
    let c = creds(Env::from_pairs([("WS", " \n")]), p);
    assert_eq!(
        c.check(&cmd(&["ws"])),
        Err("cmd printed nothing".to_owned())
    );
    assert_eq!(c.check(&file(&f)), Err("file is empty".to_owned()));
    assert_eq!(c.check(&var("WS")), Err("env WS is empty".to_owned()));
}

#[test]
fn no_source_errors() {
    let c = real();
    let none = CredentialSource::default();
    assert!(c.check(&none).is_err());
    assert!(c.read(&none).is_err());
}

#[test]
fn read_hides_value_on_error() {
    let c = real();
    assert_eq!(
        c.read(&cmd(&["printf", "v\\n"])).unwrap().as_slice(),
        b"v\n".as_slice()
    );
    let err = c
        .read(&cmd(&["sh", "-c", "echo s3cr3t; exit 3"]))
        .err()
        .unwrap();
    assert_eq!(err, "cmd exited 3");
    assert!(!err.contains("s3cr3t"));
}

#[test]
fn read_from_env_and_file() {
    let d = tempfile::TempDir::new().unwrap();
    let f = d.path().join("f");
    std::fs::write(&f, "fv\n").unwrap();
    let env = Env::from_pairs([("E", "ev")]);
    let c = creds(env, Arc::new(ScriptedProc::new()));
    assert_eq!(c.read(&var("E")).unwrap().as_slice(), b"ev".as_slice());
    assert_eq!(c.read(&file(&f)).unwrap().as_slice(), b"fv\n".as_slice());
}

#[test]
fn secret_from_env_keeps_bytes() {
    let c = creds(
        Env::from_pairs([("E", "ev")]),
        Arc::new(ScriptedProc::new()),
    );
    assert_eq!(c.secret(&var("E")).unwrap().expose(), b"ev".as_slice());
}

#[test]
fn secret_missing_or_blank_errors_without_value() {
    let c = creds(
        Env::from_pairs([("B", "  ")]),
        Arc::new(ScriptedProc::new()),
    );
    assert_eq!(
        c.secret(&var("NOPE")).err(),
        Some("env NOPE is not set".to_owned())
    );
    assert_eq!(c.secret(&var("B")).err(), Some("env B is empty".to_owned()));
}

#[test]
fn secret_trims_one_trailing_newline_from_file_and_cmd() {
    let d = tempfile::TempDir::new().unwrap();
    let f = d.path().join("f");
    std::fs::write(&f, "tok\n").unwrap();
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Sh, &exec(&["t"]), Reply::Ok("tk\r\n".into()));
    let c = creds(Env::from_pairs([("X", "y")]), p);
    assert_eq!(c.secret(&file(&f)).unwrap().expose(), b"tok".as_slice());
    assert_eq!(c.secret(&cmd(&["t"])).unwrap().expose(), b"tk".as_slice());
}

#[test]
fn real_argv_cmd_runs() {
    let c = real();
    let src = cmd(&["echo", "hi"]);
    assert_eq!(c.read(&src).unwrap().as_slice(), b"hi\n".as_slice());
}

#[test]
fn describe_never_shows_values() {
    assert_eq!(describe(&cmd(&["op", "secret"])), "cmd op");
    assert_eq!(describe(&file(Path::new("/a/b"))), "file /a/b");
    assert_eq!(describe(&var("TOK")), "env TOK");
}

#[test]
fn pass_env_lists_visible_and_unlisted_hidden() {
    let env = Env::from_pairs([("MOOR_T_LISTED", "sekret1"), ("MOOR_T_HIDDEN", "sekret2")]);
    let proc = Arc::new(SystemProc::new(&env, &ToolPaths::default()));
    let c = Creds::new(env, proc);
    let src = CredentialSource {
        pass_env: vec!["MOOR_T_LISTED".into(), "MOOR_T_UNSET".into()],
        ..cmd(&[
            "sh",
            "-c",
            "echo \"[${MOOR_T_LISTED-}][${MOOR_T_HIDDEN-}][${MOOR_T_UNSET-}]\"",
        ])
    };
    assert_eq!(
        c.read(&src).unwrap().as_slice(),
        b"[sekret1][][]\n".as_slice()
    );
    assert_eq!(
        describe(&src),
        "cmd sh (passes env MOOR_T_LISTED, MOOR_T_UNSET)"
    );
}
