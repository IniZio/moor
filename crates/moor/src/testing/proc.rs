//! Scripted `Proc` for adapter unit tests.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use moor_core::seam::proc::{
    ChildHandle, Deadline, ExitInfo, FirstLineError, Output, Proc, ProcError, ProcOutput, ProcSpec,
    Signal, StderrGate, Tool,
};
use zeroize::Zeroizing;

mod detached;
pub(crate) use detached::DaemonCall;
use detached::{call_of, Daemons};
mod once;
mod reply;
mod signal;
use signal::Signals;

/// A canned reply.
#[derive(Clone)]
pub(crate) enum Reply {
    /// Exit 0 with this stdout.
    Ok(String),
    /// Exit 0 with these raw stdout bytes; `run` decodes them lossily.
    Bytes(Vec<u8>),
    /// Exit 0 with stdout produced lazily by this reader, never held whole by `run_reading`.
    Reader(fn() -> Box<dyn std::io::Read + Send>),
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
    /// The reply this function gives for the call's arguments; not for `run_secret`.
    Computed(ReplyFn),
}

/// Picks a reply from a call's arguments.
pub(crate) type ReplyFn = Arc<dyn Fn(&[String]) -> Reply + Send + Sync>;

/// A canned outcome for a long-lived child started with `spawn_child`.
#[derive(Clone)]
pub(crate) enum ChildReply {
    /// The first stdout line is this text (without line ending).
    Ready(String),
    /// The child exits before printing a line.
    Exit { code: i32, stderr: String },
    /// The child never prints; the first-line wait times out at once.
    Hang,
    /// Prints this ready line, then exits with `code`.
    ReadyThenExit { line: String, code: i32 },
}

/// A recorded invocation.
#[derive(Clone, Debug)]
pub(crate) struct Call {
    pub tool: Tool,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
    pub deadline: Deadline,
    pub stdin: Vec<u8>,
    pub stderr_gate: Option<StderrGate>,
}

struct Rule {
    tool: Tool,
    prefix: Vec<String>,
    reply: Reply,
    /// True when the rule is dropped after its first match.
    once: bool,
}

struct ChildRule {
    tool: Tool,
    prefix: Vec<String>,
    reply: ChildReply,
    /// Answers one spawn only.
    once: bool,
}

/// First fake pid handed out by `spawn_child`.
const FIRST_CHILD_PID: u32 = 40_000;

/// Fake child; records its pid in the shared list when dropped.
struct FakeChild {
    pid: u32,
    tool: Tool,
    reply: ChildReply,
    /// Set once `first_line` has handed out the ready line.
    printed: bool,
    dropped: Arc<Mutex<Vec<u32>>>,
}

impl ChildHandle for FakeChild {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn first_line(&mut self, deadline: Deadline) -> Result<String, FirstLineError> {
        match &self.reply {
            ChildReply::Ready(line) => Ok(line.clone()),
            ChildReply::ReadyThenExit { line, .. } => {
                self.printed = true;
                Ok(line.clone())
            }
            ChildReply::Exit { code, stderr } => Err(FirstLineError::Exited {
                code: Some(*code),
                signal: None,
                stderr_tail: stderr.clone(),
            }),
            ChildReply::Hang => Err(FirstLineError::Timeout {
                tool: self.tool,
                after: deadline.0,
            }),
        }
    }

    fn try_exit(&mut self) -> Option<ExitInfo> {
        let code = match &self.reply {
            ChildReply::Exit { code, .. } => *code,
            ChildReply::ReadyThenExit { code, .. } if self.printed => *code,
            ChildReply::Ready(_) | ChildReply::Hang | ChildReply::ReadyThenExit { .. } => {
                return None;
            }
        };
        Some(ExitInfo {
            code: Some(code),
            signal: None,
        })
    }
}

impl Drop for FakeChild {
    fn drop(&mut self) {
        self.dropped
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(self.pid);
    }
}

/// Matches calls against rules in order; first match wins.
pub(crate) struct ScriptedProc {
    rules: Mutex<Vec<Rule>>,
    calls: Mutex<Vec<Call>>,
    /// Pids scripted as gone.
    dead: Mutex<Vec<u32>>,
    child_rules: Mutex<Vec<ChildRule>>,
    next_pid: AtomicU32,
    /// Pids of spawned children that were dropped.
    dropped: Arc<Mutex<Vec<u32>>>,
    daemons: Daemons,
    signals: Signals,
    boot_id: Mutex<Option<String>>,
}

impl ScriptedProc {
    pub(crate) fn new() -> Self {
        Self {
            rules: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
            dead: Mutex::new(Vec::new()),
            child_rules: Mutex::new(Vec::new()),
            next_pid: AtomicU32::new(FIRST_CHILD_PID),
            dropped: Arc::new(Mutex::new(Vec::new())),
            daemons: Daemons::default(),
            signals: Signals::default(),
            boot_id: Mutex::new(None),
        }
    }

    /// Report `id` as the host boot id; none by default.
    pub(crate) fn with_boot_id(&self, id: &str) -> &Self {
        *self.boot_id.lock().unwrap_or_else(PoisonError::into_inner) = Some(id.to_owned());
        self
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
        self.push_rule(tool, prefix, reply, false)
    }

    /// Script the child started by `spawn_child` for calls matching `prefix`.
    pub(crate) fn on_child(&self, tool: Tool, prefix: &[&str], reply: ChildReply) -> &Self {
        self.push_child_rule(tool, prefix, reply, false)
    }

    /// Like [`Self::on_child`], but answers one spawn only; later spawns fall to the next rule.
    pub(crate) fn once_child(&self, tool: Tool, prefix: &[&str], reply: ChildReply) -> &Self {
        self.push_child_rule(tool, prefix, reply, true)
    }

    /// The reply of the first child rule matching `tool` and `args`, removing a one-shot rule.
    fn child_reply(&self, tool: Tool, args: &[String]) -> Option<ChildReply> {
        let mut rules = self
            .child_rules
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let i = rules
            .iter()
            .position(|r| r.tool == tool && args.starts_with(&r.prefix))?;
        if rules.get(i)?.once {
            Some(rules.remove(i).reply)
        } else {
            rules.get(i).map(|r| r.reply.clone())
        }
    }

    fn push_child_rule(&self, tool: Tool, prefix: &[&str], reply: ChildReply, once: bool) -> &Self {
        let prefix = prefix.iter().map(|s| (*s).to_owned()).collect();
        self.child_rules
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(ChildRule {
                tool,
                prefix,
                reply,
                once,
            });
        self
    }

    /// Pids of spawned children that have been dropped.
    pub(crate) fn dropped(&self) -> Vec<u32> {
        self.dropped
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn calls(&self) -> Vec<Call> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }

    fn record(&self, spec: &ProcSpec) -> Vec<String> {
        let args = spec.arg_strings();
        if let Ok(mut c) = self.calls.lock() {
            c.push(call_of(spec));
        }
        args
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
                    Reply::Reader(open) => Some(reply::drain(*open)),
                    Reply::Fail { .. }
                    | Reply::Timeout
                    | Reply::Signalled(_)
                    | Reply::Interrupted
                    | Reply::Computed(_) => None,
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

    /// Non-zero exits come back as `NonZero` with empty stdout; the output
    /// target is not written.
    fn run_streamed(&self, spec: &ProcSpec, _out: &Output) -> Result<(), ProcError> {
        let args = self.record(spec);
        match self.reply_for(spec, &args) {
            Ok(_) => Ok(()),
            Err(ProcError::NonZero {
                code, stderr_tail, ..
            }) => Err(ProcError::NonZero {
                code,
                stdout: String::new(),
                stderr_tail,
            }),
            Err(e) => Err(e),
        }
    }

    fn run_reading(
        &self,
        spec: &ProcSpec,
        read: &mut dyn FnMut(&mut dyn std::io::Read),
    ) -> Result<(), ProcError> {
        self.reading(spec, read)
    }

    fn spawn_detached(&self, spec: &ProcSpec) -> Result<(), ProcError> {
        let args = self.record(spec);
        match self.reply_for(spec, &args) {
            Err(e @ ProcError::NonZero { code, .. }) if code != 127 => Err(e),
            _ => Ok(()),
        }
    }

    fn spawn_daemon(&self, spec: &ProcSpec, log: &Path) -> Result<u32, ProcError> {
        Ok(self.spawn_daemon_call(spec, log))
    }

    fn spawn_child(&self, spec: &ProcSpec) -> Result<Box<dyn ChildHandle>, ProcError> {
        let args = self.record(spec);
        let reply = self.child_reply(spec.tool, &args).ok_or_else(|| {
            ProcError::Spawn(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("unscripted: {} {}", spec.tool.program(), args.join(" ")),
            ))
        })?;
        Ok(Box::new(FakeChild {
            pid: self.next_pid.fetch_add(1, Ordering::Relaxed),
            tool: spec.tool,
            reply,
            printed: false,
            dropped: Arc::clone(&self.dropped),
        }))
    }

    fn signal_group(&self, pgid: u32, sig: Signal) -> Result<(), ProcError> {
        self.send_signal(pgid, sig)
    }

    fn boot_id(&self) -> Option<String> {
        self.boot_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn group_leader(&self, pid: u32) -> bool {
        self.alive(pid) && !self.is_follower(pid)
    }

    fn alive(&self, pid: u32) -> bool {
        !self
            .dead
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&pid)
            && !self
                .dropped
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(&pid)
    }
}

#[cfg(test)]
mod tests;
