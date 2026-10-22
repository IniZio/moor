//! The ssh transport: `ssh -T <host> moor controller attach`.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use moor_client::{Attempt, ATTACH_REFUSED_EXIT};
use moor_core::seam::proc::Tool;

use super::{ClientError, SessionError, Target};
use crate::config_load::load_user;
use crate::env::Env;

/// Polls of a failed ssh for its exit status, [`EXIT_POLL`] apart.
const EXIT_POLLS: u32 = 50;
/// Pause between exit status polls.
const EXIT_POLL: Duration = Duration::from_millis(10);

/// Transport over `ssh -T <host> moor controller attach`.
pub(super) fn call_ssh<T>(
    env: &Env,
    target: &Target,
    host: &str,
    exchange: impl FnOnce(&mut dyn BufRead, &mut dyn Write) -> Result<T, SessionError>,
) -> Attempt<T, ClientError> {
    let unreachable = |detail: String| ClientError::Unreachable {
        target: target.clone(),
        detail,
    };
    let spawned = Command::new(ssh_program(env))
        .args(["-T", host, "moor", "controller", "attach"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => return Attempt::Failed(unreachable(format!("cannot run ssh: {e}"))),
    };
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Attempt::Failed(ClientError::Protocol("ssh pipes unavailable".to_owned()));
    };
    let result = exchange(&mut BufReader::new(stdout), &mut stdin);
    drop(stdin);
    let result = match result {
        Ok(value) => Attempt::Done(value),
        Err(SessionError::LostBeforeHello(msg)) => {
            let status = exit_status(&mut child);
            let note = status.map_or_else(String::new, |s| format!(" (ssh {s})"));
            let e = unreachable(format!("{msg}{note}"));
            match status.and_then(|s| s.code()) {
                Some(code) if code == i32::from(ATTACH_REFUSED_EXIT) => Attempt::Refused(e),
                Some(0) => Attempt::Unrun(e),
                _ => Attempt::Failed(e),
            }
        }
        Err(SessionError::Restarting(msg)) => Attempt::Unrun(ClientError::Protocol(msg)),
        Err(SessionError::Client(e)) => Attempt::Failed(e),
    };
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// ssh program: `MOOR_SSH_BIN`, then `tools.ssh` of the user config, then `ssh` on `PATH`.
fn ssh_program(env: &Env) -> PathBuf {
    if let Some(v) = env.get(Tool::Ssh.bin_var()).filter(|v| !v.is_empty()) {
        return v.into();
    }
    load_user(env)
        .ok()
        .and_then(|c| c.tools.ssh)
        .unwrap_or_else(|| Tool::Ssh.program().into())
}

/// The exit status of `child` once it has exited, polled for a short while.
fn exit_status(child: &mut Child) -> Option<ExitStatus> {
    for _ in 0..EXIT_POLLS {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        std::thread::sleep(EXIT_POLL);
    }
    None
}
