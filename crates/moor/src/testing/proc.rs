//! Scripted `Proc` for adapter unit tests.

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use moor_core::seam::proc::{Deadline, Output, Proc, ProcError, ProcOutput, ProcSpec, Tool};
use zeroize::Zeroizing;

/// A canned reply.
pub(crate) enum Reply {
    /// Exit 0 with this stdout.
    Ok(String),
    /// Exit 0 with these raw stdout bytes; `run` decodes them lossily.
    Bytes(Vec<u8>),
    /// Non-zero exit.
    Fail {
        code: i32,
        stdout: String,
        stderr: String,
    },
    /// Deadline exceeded.
    Timeout,
    /// Killed by this signal.
    Signalled(i32),
    /// moor was interrupted.
    Interrupted,
}

/// A recorded invocation.
#[derive(Clone, Debug)]
pub(crate) struct Call {
    pub tool: Tool,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
    pub deadline: Deadline,
}

struct Rule {
    tool: Tool,
    prefix: Vec<String>,
    reply: Reply,
}

/// Matches calls against rules in order; first match wins.
pub(crate) struct ScriptedProc {
    rules: Mutex<Vec<Rule>>,
    calls: Mutex<Vec<Call>>,
    /// Pids scripted as gone.
    dead: Mutex<Vec<u32>>,
}

impl ScriptedProc {
    pub(crate) fn new() -> Self {
        Self {
            rules: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
            dead: Mutex::new(Vec::new()),
        }
    }

    /// Script `pid` as gone; every other pid is alive.
    pub(crate) fn dead(&self, pid: u32) -> &Self {
        self.dead
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(pid);
        self
    }

    pub(crate) fn on(&self, tool: Tool, prefix: &[&str], reply: Reply) -> &Self {
        let prefix = prefix.iter().map(|s| (*s).to_owned()).collect();
        if let Ok(mut r) = self.rules.lock() {
            r.push(Rule {
                tool,
                prefix,
                reply,
            });
        }
        self
    }

    pub(crate) fn calls(&self) -> Vec<Call> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }

    fn record(&self, spec: &ProcSpec) -> Vec<String> {
        let args = spec.arg_strings();
        if let Ok(mut c) = self.calls.lock() {
            c.push(Call {
                tool: spec.tool,
                args: args.clone(),
                env: spec.env.extra.clone(),
                cwd: spec.cwd.clone(),
                deadline: spec.deadline,
            });
        }
        args
    }

    fn reply_for(&self, spec: &ProcSpec, args: &[String]) -> Result<ProcOutput, ProcError> {
        let rules = self.rules.lock().map_err(|_| poisoned())?;
        let hit = rules
            .iter()
            .find(|r| r.tool == spec.tool && args.starts_with(&r.prefix));
        let out = match hit.map(|r| &r.reply) {
            Some(Reply::Ok(out)) => Ok(ProcOutput {
                code: 0,
                stdout: out.clone(),
                stderr: String::new(),
                truncated: false,
            }),
            Some(Reply::Bytes(b)) => Ok(ProcOutput {
                code: 0,
                stdout: String::from_utf8_lossy(b).into_owned(),
                stderr: String::new(),
                truncated: false,
            }),
            Some(Reply::Fail {
                code,
                stdout,
                stderr,
            }) => Err(ProcError::NonZero {
                code: *code,
                stdout: stdout.clone(),
                stderr_tail: stderr.clone(),
            }),
            Some(Reply::Signalled(sig)) => Err(ProcError::Signalled(*sig)),
            Some(Reply::Interrupted) => Err(ProcError::Interrupted),
            Some(Reply::Timeout) => Err(ProcError::Timeout {
                tool: spec.tool,
                after: spec.deadline.0,
            }),
            None => Err(ProcError::NonZero {
                code: 127,
                stdout: String::new(),
                stderr_tail: format!("unscripted: {} {}", spec.tool.program(), args.join(" ")),
            }),
        };
        drop(rules);
        out
    }
}

fn poisoned() -> ProcError {
    ProcError::Spawn(std::io::Error::other("fake proc lock poisoned"))
}

impl Proc for ScriptedProc {
    fn run(&self, spec: &ProcSpec) -> Result<ProcOutput, ProcError> {
        let args = self.record(spec);
        self.reply_for(spec, &args)
    }

    fn run_secret(&self, spec: &ProcSpec) -> Result<Zeroizing<Vec<u8>>, ProcError> {
        let args = self.record(spec);
        let bytes = self.rules.lock().ok().and_then(|rules| {
            rules
                .iter()
                .find(|r| r.tool == spec.tool && args.starts_with(&r.prefix))
                .and_then(|r| match &r.reply {
                    Reply::Bytes(b) => Some(b.clone()),
                    Reply::Ok(s) => Some(s.clone().into_bytes()),
                    Reply::Fail { .. }
                    | Reply::Timeout
                    | Reply::Signalled(_)
                    | Reply::Interrupted => None,
                })
        });
        bytes
            .map(Zeroizing::new)
            .ok_or_else(|| match self.reply_for(spec, &args) {
                Err(ProcError::NonZero {
                    code, stderr_tail, ..
                }) => ProcError::NonZero {
                    code,
                    stdout: String::new(),
                    stderr_tail,
                },
                Err(e) => e,
                Ok(_) => poisoned(),
            })
    }

    /// Non-zero exits come back as `Ok(code)`; the output target is not written.
    fn run_streamed(&self, spec: &ProcSpec, _out: &Output) -> Result<i32, ProcError> {
        let args = self.record(spec);
        match self.reply_for(spec, &args) {
            Ok(o) => Ok(o.code),
            Err(ProcError::NonZero { code, .. }) => Ok(code),
            Err(e) => Err(e),
        }
    }

    fn spawn_detached(&self, spec: &ProcSpec) -> Result<(), ProcError> {
        let args = self.record(spec);
        match self.reply_for(spec, &args) {
            Err(e @ ProcError::NonZero { code, .. }) if code != 127 => Err(e),
            _ => Ok(()),
        }
    }

    fn alive(&self, pid: u32) -> bool {
        !self
            .dead
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&pid)
    }
}

#[cfg(test)]
mod tests;
