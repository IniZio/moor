//! What the agent does with each stream the runner opens: an exec, a hook, or a relay, in the
//! Box's dir under this account's home.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use moor_client::link::ops::{self, Op, Reply, MAX_TIMEOUT_SECS};
use moor_client::link::Channel;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::files;

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
        }
    }

    /// `/bin/sh -c`, in the home with this machine's home and shell.
    fn carrier(&self) -> Command {
        let mut sh = Command::new("/bin/sh");
        sh.current_dir(&self.home)
            .env("HOME", &self.home)
            .env("SHELL", &self.shell)
            .arg("-c");
        sh
    }
}

/// Serves the request that opens `ch`.
pub(crate) async fn serve(machine: &Machine, mut ch: Channel) {
    let reply = match ops::read_line::<_, Op>(&mut ch).await {
        Err(e) => refused(e),
        Ok(Op::Sync { box_id, argv }) => return files::sync(machine, ch, box_id, &argv).await,
        Ok(Op::Pull { box_id, path }) => return files::pull(machine, ch, box_id, &path).await,
        Ok(op) => match start(machine, op) {
            Ok(Start::Run(script, timeout)) => return ran(machine, ch, &script, timeout).await,
            Ok(Start::Relay(script)) => return relay(machine, ch, &script).await,
            Err(reply) => reply,
        },
    };
    let _ = ops::write_line(&mut ch, &reply).await;
    let _ = ch.shutdown().await;
}

/// What a request starts.
enum Start {
    /// A script to run within a timeout.
    Run(String, Duration),
    /// A relay's script, whose stdio the stream carries.
    Relay(String),
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
            let path = file(&machine.hooks.join(hook.file())).ok_or(Reply::Missing)?;
            let body = ops::in_box(box_id, &captured(&path));
            Ok(Start::Run(ops::script(&env, &body), timeout))
        }
        Op::Relay { box_id, name, env } => {
            checked(box_id, 1)
                .and_then(|_| relay_name(&name))
                .map_err(refused)?;
            let path = file(&machine.hooks.join("relays").join(&name)).ok_or(Reply::Missing)?;
            let body = format!("exec \"$SHELL\" -lc {}", ops::quote(&ops::quote(&path)));
            Ok(Start::Relay(ops::script(&env, &ops::in_box(box_id, &body))))
        }
        Op::Sync { .. } | Op::Pull { .. } => Err(refused("not a command".to_owned())),
    }
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
