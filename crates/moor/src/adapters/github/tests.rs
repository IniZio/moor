use std::sync::Arc;

use moor_core::seam::forge::{Forge, ForgeError, ForgeRepo};
use moor_core::seam::proc::Tool;

use super::GitHub;
use crate::testing::proc::{Reply, ScriptedProc};

const KEY: &str = "github.you_token_cmd";

fn gh(reply: Reply) -> (GitHub, Arc<ScriptedProc>) {
    let proc = Arc::new(ScriptedProc::new());
    proc.on(Tool::Sh, &["-c"], reply);
    let cmd = ["gh", "auth", "token"].map(str::to_owned).to_vec();
    let env = vec![("GH_HOST".to_owned(), "github.com".to_owned())];
    (GitHub::new(proc.clone(), cmd, env), proc)
}

fn token(g: &GitHub) -> Result<Vec<u8>, ForgeError> {
    g.repo_token(&ForgeRepo::new("o", "n"))
        .map(|s| s.expose().to_vec())
}

fn fail(code: i32) -> Reply {
    Reply::Fail {
        code,
        stdout: "SECRETOUT".to_owned(),
        stderr: "SECRETERR".to_owned(),
    }
}

#[test]
fn token_is_trimmed_and_command_is_passed_through() {
    let (g, proc) = gh(Reply::Ok("tok123\n".to_owned()));
    assert_eq!(token(&g), Ok(b"tok123".to_vec()));
    let calls = proc.calls();
    let [call] = calls.as_slice() else {
        return assert_eq!(calls.len(), 1);
    };
    assert_eq!(
        call.args,
        [
            "-c",
            "exec \"$@\"",
            "moor-github-token",
            "gh",
            "auth",
            "token"
        ]
    );
    assert_eq!(call.env, [("GH_HOST".to_owned(), "github.com".to_owned())]);
}

#[test]
fn nonzero_exit_is_auth_without_leaking_output() {
    let (g, _) = gh(fail(1));
    let m = match token(&g) {
        Err(ForgeError::Auth(m)) => m,
        other => format!("unexpected: {other:?}"),
    };
    assert!(!m.starts_with("unexpected"), "{m}");
    assert!(m.contains(KEY));
    assert!(!m.contains("SECRETOUT") && !m.contains("SECRETERR"));
}

#[test]
fn missing_binary_is_failed() {
    for code in [126, 127] {
        let (g, _) = gh(fail(code));
        let m = match token(&g) {
            Err(ForgeError::Failed(m)) => m,
            other => format!("unexpected: {other:?}"),
        };
        assert!(!m.starts_with("unexpected"), "{m}");
        assert!(m.contains(KEY));
        assert!(!m.contains("SECRETOUT"));
    }
}

#[test]
fn blank_output_is_auth() {
    let (g, _) = gh(Reply::Ok("  \n".to_owned()));
    let m = match token(&g) {
        Err(ForgeError::Auth(m)) => m,
        other => format!("unexpected: {other:?}"),
    };
    assert!(!m.starts_with("unexpected"), "{m}");
    assert!(m.contains(KEY));
}

#[test]
fn timeout_signal_and_interrupt_are_failed() {
    for reply in [Reply::Timeout, Reply::Signalled(9), Reply::Interrupted] {
        let (g, _) = gh(reply);
        let m = match token(&g) {
            Err(ForgeError::Failed(m)) => m,
            other => format!("unexpected: {other:?}"),
        };
        assert!(!m.starts_with("unexpected"), "{m}");
        assert!(m.contains(KEY));
    }
}

#[test]
fn routes_and_repo_of_delegate() {
    let (g, _) = gh(Reply::Ok(String::new()));
    assert_eq!(g.routes().len(), 2);
    assert_eq!(
        g.repo_of("git@github.com:o/n.git"),
        Some(ForgeRepo::new("o", "n"))
    );
}
