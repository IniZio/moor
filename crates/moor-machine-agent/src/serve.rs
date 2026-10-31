//! What the agent does with each stream the runner opens: an exec, a hook, or a relay, in the
//! Box's dir under this account's home; a sweep of closed Boxes' dirs; a lease or dial on a
//! pool host; or a status.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use moor_client::link::ops::{self, Hook, Op, Reply, MAX_TIMEOUT_SECS};
use moor_client::link::sweep::Swept;
use moor_client::link::Channel;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::files;
use crate::listen::{self, Link};
use crate::pool::{self, Pool};
use crate::status::{self, Recent};
use crate::sweep;

/// How long a relay may take to exit once its stdin closes, before it is killed.
const RELAY_EXIT: Duration = Duration::from_secs(5);

/// Where commands run and where the machine's own hooks are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    /// The account's home, under which each Box's dir lies.
    pub home: PathBuf,
    /// The login shell commands run through.
    pub shell: PathBuf,
    /// `prepare`, `suspend`, `release`, and `relays/<name>`.
    pub hooks: PathBuf,
    /// The pool it serves, when it is a pool host.
    pub pool: Option<Pool>,
    /// Whether it runs `exec` scripts; off unless the operator asked for it.
    pub allow_exec: bool,
    /// How old a Box's folders must be before a sweep may remove them.
    pub sweep_grace: Duration,
}

impl Machine {
    /// This account's: `$HOME`, `$SHELL`, and `$XDG_CONFIG_HOME/moor-machine-agent/hooks`.
    #[must_use]
    pub fn from_env() -> Self {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let config =
            std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
        Self {
            shell: std::env::var_os("SHELL").map_or_else(|| "/bin/sh".into(), PathBuf::from),
            hooks: config.join("moor-machine-agent/hooks"),
            home,
            pool: None,
            allow_exec: false,
            sweep_grace: sweep::GRACE,
        }
    }

    /// `/bin/sh -c`, in the home with this machine's home and shell.
    pub(crate) fn carrier(&self) -> Command {
        let mut sh = Command::new("/bin/sh");
        sh.current_dir(&self.home)
            .env("HOME", &self.home)
            .env("SHELL", &self.shell)
            .arg("-c");
        sh
    }
}

/// Serves the request that opens `ch`, when this machine's mode allows it, and notes it in
/// `recent`. Every agent serves a status, which is read-only; past that, this match is the
/// allow-list: a pool host serves only lease and dial, any other machine everything else, and
/// `exec` only when it was started with `--allow-exec`.
pub(crate) async fn serve(machine: &Machine, link: &Link, recent: &Recent, mut ch: Channel) {
    let op = match ops::read_line::<_, Op>(&mut ch).await {
        Ok(op) => op,
        Err(e) => return end(ch, refused(e)).await,
    };
    if op == Op::Status {
        let reply = Reply::Status(Box::new(status::status(machine, recent).await));
        return end(ch, reply).await;
    }
    if machine.pool.is_some() && matches!(op, Op::Sweep { .. }) {
        return end(ch, Reply::Swept(Swept::default())).await;
    }
    let (name, target) = (kind(&op), target(&op));
    let failed = dispatch(machine, link, ch, op).await;
    recent.record(name, target, failed.as_deref());
}

/// Serves `op` when this machine's mode allows it; why it failed.
async fn dispatch(machine: &Machine, link: &Link, ch: Channel, op: Op) -> Option<String> {
    match (machine.pool.as_ref(), op) {
        (Some(pool), op @ (Op::Lease { .. } | Op::Dial { .. })) => {
            serve_pool(&machine.home, pool, ch, op).await
        }
        (
            None,
            op @ (Op::Exec { .. }
            | Op::Hook { .. }
            | Op::Relay { .. }
            | Op::Sync { .. }
            | Op::Pull { .. }
            | Op::Listen { .. }
            | Op::Sweep { .. }
            | Op::Remove { .. }),
        ) if machine.allow_exec || !matches!(op, Op::Exec { .. }) => {
            serve_machine(machine, link, ch, op).await
        }
        (pool, op) => {
            let why = not_served(pool.is_some(), &op);
            end(ch, refused(why.clone())).await;
            Some(why)
        }
    }
}

/// Why this machine does not serve `op`.
fn not_served(pool_host: bool, op: &Op) -> String {
    match (pool_host, op) {
        (true, _) => format!("a pool host does not serve {}", kind(op)),
        (false, Op::Exec { .. }) => {
            "this machine was not started with --allow-exec, so it does not serve exec".to_owned()
        }
        (false, _) => format!(
            "this machine is not a pool host, so it does not serve {}",
            kind(op)
        ),
    }
}

/// Serves a lease or a dial for `pool`; why it failed.
async fn serve_pool(home: &Path, pool: &Pool, ch: Channel, op: Op) -> Option<String> {
    let reply = match op {
        Op::Lease { verb, slug, key } => {
            pool::lease(home, pool, verb, slug.as_deref(), key.as_deref()).await
        }
        Op::Dial { slug } => match pool::dial(home, pool, &slug).await {
            Ok(sshd) => {
                pool::carry(ch, sshd).await;
                return None;
            }
            Err(reason) => refused(reason),
        },
        other => refused(format!("{} is not a pool op", kind(&other))),
    };
    let failed = refusal(&reply);
    end(ch, reply).await;
    failed
}

/// Serves an op of a machine that is not a pool host; why it was refused.
async fn serve_machine(machine: &Machine, link: &Link, ch: Channel, op: Op) -> Option<String> {
    match op {
        Op::Sync { box_id, argv } => files::sync(machine, ch, box_id, &argv).await,
        Op::Pull { box_id, path } => files::pull(machine, ch, box_id, &path).await,
        Op::Sweep { keep } => return sweep::serve(machine, ch, &keep).await,
        Op::Remove { box_id } => return sweep::serve_remove(machine, ch, box_id).await,
        op => match start(machine, op) {
            Ok(Start::Run(script, timeout)) => ran(machine, ch, &script, timeout).await,
            Ok(Start::Relay(script)) => relay(machine, ch, &script).await,
            Ok(Start::Listen {
                box_id,
                listen,
                ports,
            }) => listen::serve(ch, link, box_id, listen, &ports).await,
            Err(reply) => {
                let failed = refusal(&reply);
                end(ch, reply).await;
                return failed;
            }
        },
    }
    None
}

/// Why `reply` refused its op.
fn refusal(reply: &Reply) -> Option<String> {
    match reply {
        Reply::Refused { reason } => Some(reason.clone()),
        _ => None,
    }
}

/// The Box id or lease slug `op` names.
fn target(op: &Op) -> Option<String> {
    match op {
        Op::Exec { box_id, .. }
        | Op::Hook { box_id, .. }
        | Op::Relay { box_id, .. }
        | Op::Sync { box_id, .. }
        | Op::Pull { box_id, .. }
        | Op::Listen { box_id, .. }
        | Op::Remove { box_id } => Some(box_id.to_string()),
        Op::Lease { slug, .. } => slug.clone(),
        Op::Dial { slug } => Some(slug.clone()),
        Op::Sweep { .. } | Op::Status => None,
    }
}

/// Answers `reply` on `ch` and closes it.
pub(crate) async fn end(mut ch: Channel, reply: Reply) {
    let _ = ops::write_line(&mut ch, &reply).await;
    let _ = ch.shutdown().await;
}

/// The op's name on the wire.
const fn kind(op: &Op) -> &'static str {
    match op {
        Op::Exec { .. } => "exec",
        Op::Hook { .. } => "hook",
        Op::Relay { .. } => "relay",
        Op::Sync { .. } => "sync",
        Op::Pull { .. } => "pull",
        Op::Listen { .. } => "listen",
        Op::Lease { .. } => "lease",
        Op::Dial { .. } => "dial",
        Op::Sweep { .. } => "sweep",
        Op::Remove { .. } => "remove",
        Op::Status => "status",
    }
}

/// What a request starts.
enum Start {
    /// A script to run within a timeout.
    Run(String, Duration),
    /// A relay's script, whose stdio the stream carries.
    Relay(String),
    /// Ports to listen on for a Box while the stream stays open.
    Listen {
        /// The Box.
        box_id: i64,
        /// The listen's id.
        listen: u64,
        /// The machine ports.
        ports: Vec<u16>,
    },
}

/// What `op` starts, or the reply that refuses it.
fn start(machine: &Machine, op: Op) -> Result<Start, Reply> {
    match op {
        Op::Exec {
            box_id,
            env,
            script,
            timeout_secs,
        } => {
            let timeout = checked(box_id, timeout_secs).map_err(refused)?;
            Ok(Start::Run(
                ops::script(&env, &ops::in_box(box_id, &script)),
                timeout,
            ))
        }
        Op::Hook {
            box_id,
            hook,
            env,
            timeout_secs,
        } => {
            let timeout = checked(box_id, timeout_secs).map_err(refused)?;
            let script = hook_script(machine, box_id, hook, &env).ok_or(Reply::Missing)?;
            Ok(Start::Run(script, timeout))
        }
        Op::Relay { box_id, name, env } => {
            checked(box_id, 1)
                .and_then(|_| relay_name(&name))
                .map_err(refused)?;
            let path = file(&machine.hooks.join("relays").join(&name)).ok_or(Reply::Missing)?;
            let body = format!("exec \"$SHELL\" -lc {}", ops::quote(&ops::quote(&path)));
            Ok(Start::Relay(ops::script(&env, &ops::in_box(box_id, &body))))
        }
        Op::Sync { .. }
        | Op::Pull { .. }
        | Op::Sweep { .. }
        | Op::Remove { .. }
        | Op::Lease { .. }
        | Op::Dial { .. }
        | Op::Status => Err(refused("not a command".to_owned())),
        Op::Listen {
            box_id,
            listen,
            ports,
        } => Ok(Start::Listen {
            box_id,
            listen,
            ports,
        }),
    }
}

/// The script that runs the machine's `hook` for Box `box_id` with `env`; none when the machine
/// has no such hook.
pub(crate) fn hook_script(
    machine: &Machine,
    box_id: i64,
    hook: Hook,
    env: &[(String, String)],
) -> Option<String> {
    let path = file(&machine.hooks.join(hook.file()))?;
    Some(ops::script(env, &ops::in_box(box_id, &captured(&path))))
}

/// A `refused` reply giving `reason`.
const fn refused(reason: String) -> Reply {
    Reply::Refused { reason }
}

/// The timeout, when `box_id` names a Box and `secs` is 1 to [`MAX_TIMEOUT_SECS`].
fn checked(box_id: i64, secs: u64) -> Result<Duration, String> {
    if box_id <= 0 {
        return Err(format!("{box_id} is not a Box id"));
    }
    if secs == 0 || secs > MAX_TIMEOUT_SECS {
        return Err(format!(
            "timeout_secs must be 1 to {MAX_TIMEOUT_SECS}, not {secs}"
        ));
    }
    Ok(Duration::from_secs(secs))
}

/// Ok when `name` is `[a-z][a-z0-9-]*`, so it names a file in `relays` and nothing else.
fn relay_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let good = chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if good {
        Ok(())
    } else {
        Err(format!("{name:?} is not a relay name"))
    }
}

/// `path` when it is a file.
fn file(path: &Path) -> Option<String> {
    path.is_file().then(|| path.to_string_lossy().into_owned())
}

/// The hook at `path`, its stdout held back until it exits so `prepare`'s facts come last.
fn captured(path: &str) -> String {
    format!(
        "f=$(mktemp) || exit 1; ( {} ) >\"$f\"; rc=$?; cat \"$f\"; rm -f \"$f\"; exit $rc",
        ops::quote(path)
    )
}

/// Runs `script` and answers what it did; the runner closing the stream, or sending anything
/// more, ends it at once.
async fn ran(machine: &Machine, ch: Channel, script: &str, timeout: Duration) {
    let (mut rd, mut wr) = tokio::io::split(ch);
    let mut probe = [0u8; 1];
    tokio::select! {
        out = ops::run(machine.carrier(), script, timeout) => {
            let reply = out.map_or_else(|e| refused(format!("cannot run the shell: {e}")), Reply::Ran);
            let _ = ops::write_line(&mut wr, &reply).await;
            let _ = wr.shutdown().await;
        }
        _ = rd.read(&mut probe) => {}
    }
}

/// Starts the relay `script` and carries its stdio on `ch` until either end closes.
async fn relay(machine: &Machine, mut ch: Channel, script: &str) {
    let spawned = machine
        .carrier()
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            let _ = ops::write_line(&mut ch, &refused(format!("cannot start it: {e}"))).await;
            return;
        }
    };
    let (Some(mut stdin), Some(mut stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return;
    };
    if ops::write_line(&mut ch, &Reply::Started).await.is_err() {
        return;
    }
    let (mut rd, mut wr) = tokio::io::split(ch);
    let to = async move {
        let _ = tokio::io::copy(&mut rd, &mut stdin).await;
    };
    let from = async move {
        let _ = tokio::io::copy(&mut stdout, &mut wr).await;
        let _ = wr.shutdown().await;
    };
    tokio::join!(to, from);
    if tokio::time::timeout(RELAY_EXIT, child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
}
