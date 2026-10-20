use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::thread::JoinHandle;

use moor_core::controller::PROTOCOL;
use moor_core::{ExitClass, Request, ShowReq};

use super::*;

fn show() -> Request {
    Request::Show(ShowReq {
        box_name: None,
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    })
}

fn ok_envelope() -> String {
    serde_json::to_string(&Response::ok("show", None)).unwrap()
}

/// Accepts one connection; for each scripted step reads one client line, then writes the
/// step's lines. An empty step list closes right after the accept.
fn serve(socket: &Path, steps: Vec<Vec<String>>) -> JoinHandle<Vec<String>> {
    let listener = UnixListener::bind(socket).unwrap();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        let mut seen = Vec::new();
        for step in steps {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            seen.push(line.trim_end().to_owned());
            for out in step {
                writeln!(writer, "{out}").unwrap();
            }
        }
        seen
    })
}

fn hello() -> String {
    format!(r#"{{"frame":"hello","protocol":{PROTOCOL}}}"#)
}

#[test]
fn happy_path_collects_progress_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let server = serve(
        &sock,
        vec![
            vec![hello()],
            vec![
                r#"{"frame":"progress","id":"1","message":"one"}"#.to_owned(),
                r#"{"frame":"progress","id":"1","message":"two"}"#.to_owned(),
                format!(
                    r#"{{"frame":"response","id":"1","envelope":{}}}"#,
                    ok_envelope()
                ),
            ],
        ],
    );
    let mut seen = Vec::new();
    let resp = call(&Env::default(), &Target::Socket(sock), &show(), |m| {
        seen.push(m.to_owned());
    })
    .unwrap();
    assert_eq!(seen, ["one", "two"]);
    assert_eq!(resp.cmd, "show");
    assert_eq!(resp.class, ExitClass::Ok);
    let lines = server.join().unwrap();
    assert_eq!(lines.first().unwrap(), &hello());
    let req = lines.get(1).unwrap();
    assert!(req.contains(r#""frame":"request""#) && req.contains(r#""id":"1""#));
}

#[test]
fn unreachable_socket_maps_to_failed_with_hint() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("missing.sock");
    let err = call(&Env::default(), &Target::Socket(sock), &show(), |_| {}).unwrap_err();
    assert!(matches!(err, ClientError::Unreachable { .. }), "{err}");
    let resp = failure_response("show", &err);
    assert_eq!(resp.class, ExitClass::Failed);
    let view = resp.error.unwrap();
    assert_eq!(view.kind, ErrorKind::Failed);
    assert!(view.message.contains("missing.sock"), "{}", view.message);
    let hint = view.hints.first().unwrap();
    assert!(hint.contains("moor controller serve --config <file>"));
    assert!(hint.contains("unset MOOR_CONTROLLER"));
}

#[test]
fn error_frame_is_protocol_error() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let server = serve(
        &sock,
        vec![
            vec![hello()],
            vec![r#"{"frame":"error","id":"1","kind":"malformed","message":"bad"}"#.to_owned()],
        ],
    );
    let err = call(&Env::default(), &Target::Socket(sock), &show(), |_| {}).unwrap_err();
    assert!(
        matches!(&err, ClientError::Protocol(m) if m.contains("bad")),
        "{err}"
    );
    server.join().unwrap();
}

#[test]
fn hello_mismatch_is_protocol_error() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let server = serve(
        &sock,
        vec![vec![r#"{"frame":"hello","protocol":9}"#.to_owned()]],
    );
    let err = call(&Env::default(), &Target::Socket(sock), &show(), |_| {}).unwrap_err();
    assert!(matches!(err, ClientError::Protocol(_)), "{err}");
    server.join().unwrap();
}

#[test]
fn eof_before_response_is_protocol_error() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let server = serve(&sock, vec![vec![hello()], vec![]]);
    let err = call(&Env::default(), &Target::Socket(sock), &show(), |_| {}).unwrap_err();
    assert!(matches!(err, ClientError::Protocol(_)), "{err}");
    server.join().unwrap();
}

#[test]
fn env_var_beats_config_and_config_is_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.yaml");
    std::fs::write(&cfg, "controller:\n  socket: ~/from-config.sock\n").unwrap();
    let cfg_s = cfg.to_str().unwrap();
    let home = dir.path().to_str().unwrap();

    let both = Env::from_pairs([
        ("MOOR_CONFIG", cfg_s),
        ("HOME", home),
        ("MOOR_CONTROLLER", "/run/env.sock"),
    ]);
    assert_eq!(
        configured_target(&both),
        Some(Target::Socket(PathBuf::from("/run/env.sock")))
    );

    let cfg_only = Env::from_pairs([("MOOR_CONFIG", cfg_s), ("HOME", home)]);
    assert_eq!(
        configured_target(&cfg_only),
        Some(Target::Socket(dir.path().join("from-config.sock")))
    );

    let empty_var = Env::from_pairs([
        ("MOOR_CONFIG", cfg_s),
        ("HOME", home),
        ("MOOR_CONTROLLER", ""),
    ]);
    assert_eq!(configured_target(&empty_var), configured_target(&cfg_only));
}

#[test]
fn unloadable_config_yields_none() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.yaml");
    std::fs::write(&cfg, "controller: [not, a, map\n").unwrap();
    let env = Env::from_pairs([("MOOR_CONFIG", cfg.to_str().unwrap())]);
    assert_eq!(configured_target(&env), None);
}

/// Writes an executable fake ssh that runs `body` and returns an Env pointing at it.
fn fake_ssh(dir: &Path, body: &str) -> Env {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("ssh");
    std::fs::write(&bin, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    Env::from_pairs([("MOOR_SSH_BIN", bin.to_str().unwrap())])
}

const ARGS_CHECK: &str = r#"[ "$*" = "-T dev@box moor controller attach" ] || exit 64"#;

fn ssh_target() -> Target {
    Target::Ssh("dev@box".to_owned())
}

#[test]
fn ssh_happy_path_runs_attach_and_collects_progress() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!(
        "{ARGS_CHECK}\nread l\necho '{}'\nread l\necho '{}'\necho '{}'",
        hello(),
        r#"{"frame":"progress","id":"1","message":"one"}"#,
        r#"{"frame":"response","id":"1","envelope":ENV}"#.replace("ENV", &ok_envelope()),
    );
    let env = fake_ssh(dir.path(), &body);
    let mut seen = Vec::new();
    let resp = call(&env, &ssh_target(), &show(), |m| seen.push(m.to_owned())).unwrap();
    assert_eq!(seen, ["one"]);
    assert_eq!(resp.class, ExitClass::Ok);
}

#[test]
fn ssh_failure_names_host_and_maps_to_failed() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_ssh(dir.path(), "exit 255");
    let err = call(&env, &ssh_target(), &show(), |_| {}).unwrap_err();
    assert!(matches!(err, ClientError::Unreachable { .. }), "{err}");
    let resp = failure_response("show", &err);
    assert_eq!(resp.class, ExitClass::Failed);
    let view = resp.error.unwrap();
    assert!(view.message.contains("dev@box"), "{}", view.message);
    assert!(view.message.contains("255"), "{}", view.message);
    assert!(view
        .hints
        .first()
        .unwrap()
        .contains("ssh dev@box moor controller attach"));
}

#[test]
fn ssh_missing_binary_is_unreachable() {
    let env = Env::from_pairs([("MOOR_SSH_BIN", "/nonexistent/ssh")]);
    let err = call(&env, &ssh_target(), &show(), |_| {}).unwrap_err();
    assert!(
        matches!(&err, ClientError::Unreachable { .. }) && err.to_string().contains("dev@box"),
        "{err}"
    );
}

#[test]
fn ssh_protocol_mismatch_names_both_versions() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_ssh(
        dir.path(),
        r#"read l; echo '{"frame":"hello","protocol":9}'"#,
    );
    let err = call(&env, &ssh_target(), &show(), |_| {}).unwrap_err();
    let msg = err.to_string();
    assert!(
        matches!(err, ClientError::Protocol(_))
            && msg.contains('9')
            && msg.contains(&PROTOCOL.to_string()),
        "{msg}"
    );
}

#[test]
fn ssh_in_config_yields_ssh_target_and_env_still_wins() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.yaml");
    std::fs::write(&cfg, "controller:\n  ssh: dev@box\n").unwrap();
    let cfg_s = cfg.to_str().unwrap();
    let env = Env::from_pairs([("MOOR_CONFIG", cfg_s)]);
    assert_eq!(configured_target(&env), Some(ssh_target()));
    let env = Env::from_pairs([("MOOR_CONFIG", cfg_s), ("MOOR_CONTROLLER", "/run/e.sock")]);
    assert_eq!(
        configured_target(&env),
        Some(Target::Socket(PathBuf::from("/run/e.sock")))
    );
}
