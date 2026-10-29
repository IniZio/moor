//! rsync's remote shell for a linked machine: `moor-machine-runner rsh <box>` hands the remote
//! `rsync --server` command to the runner over a local socket, and the runner carries it to the
//! agent on a `sync` stream. Only a Box whose sync is under way is served.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use moor_client::link::ops::{self, Op, Reply};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};

use crate::link::{self, Answer};
use crate::Agents;

/// The env var naming the runner's rsh socket, which rsync passes on to its remote shell.
pub(crate) const SOCKET_ENV: &str = "MOOR_RSH_SOCKET";

/// What the helper asks the runner for: one line on the socket.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ask {
    /// The Box being synced.
    box_id: i64,
    /// The remote command rsync gave its shell.
    argv: Vec<String>,
}

/// The Boxes whose sync to a linked machine is under way, with that machine's id and name.
#[derive(Clone, Default)]
pub(crate) struct Syncs(Arc<Mutex<HashMap<i64, (i64, String)>>>);

/// A sync under way, until it is dropped.
pub(crate) struct Syncing(Syncs, i64);

impl Drop for Syncing {
    fn drop(&mut self) {
        self.0.held().remove(&self.1);
    }
}

impl Syncs {
    /// Serves Box `box_id`'s rsh requests on machine `machine_id` while the result is held.
    pub(crate) fn start(&self, box_id: i64, machine_id: i64, machine: &str) -> Syncing {
        self.held().insert(box_id, (machine_id, machine.to_owned()));
        Syncing(self.clone(), box_id)
    }

    /// The machine Box `box_id` is syncing to.
    fn machine(&self, box_id: i64) -> Option<(i64, String)> {
        self.held().get(&box_id).cloned()
    }

    fn held(&self) -> std::sync::MutexGuard<'_, HashMap<i64, (i64, String)>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Where the helper reaches the runner, and the syncs it serves there.
#[derive(Clone)]
pub(crate) struct Rsh {
    /// The runner's socket for helpers.
    pub(crate) socket: PathBuf,
    /// The syncs under way.
    pub(crate) syncs: Syncs,
}

/// Serves helpers on a socket under `state_dir` when the runner takes agent links.
pub(crate) fn listen(state_dir: &Path, agents: Option<&Agents>) -> io::Result<Option<Rsh>> {
    let Some(agents) = agents else {
        return Ok(None);
    };
    let socket = state_dir.join("rsh.sock");
    std::fs::create_dir_all(state_dir)?;
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)?;
    let syncs = Syncs::default();
    tokio::spawn(serve(listener, agents.clone(), syncs.clone()));
    Ok(Some(Rsh { socket, syncs }))
}

/// Accepts helpers on `listener`, carrying each to its Box's machine.
async fn serve(listener: UnixListener, agents: Agents, syncs: Syncs) {
    while let Ok((stream, _)) = listener.accept().await {
        let (agents, syncs) = (agents.clone(), syncs.clone());
        tokio::spawn(async move { carry(stream, &agents, &syncs).await });
    }
}

/// One helper's stream: the agent's answer, then the rsync protocol both ways.
async fn carry(mut stream: UnixStream, agents: &Agents, syncs: &Syncs) {
    let opened = match ops::read_line::<_, Ask>(&mut stream).await {
        Err(e) => Err(e),
        Ok(ask) => match syncs.machine(ask.box_id) {
            None => Err(format!("Box {} has no sync under way", ask.box_id)),
            Some((machine_id, machine)) => {
                let op = Op::Sync {
                    box_id: ask.box_id,
                    argv: ask.argv,
                };
                match link::send(Some(agents), &machine, machine_id, &op).await {
                    Ok(Answer::Started(ch)) => Ok(ch),
                    Ok(_) => Err(format!("machine {machine} answered no sync")),
                    Err(e) => Err(e),
                }
            }
        },
    };
    match opened {
        Ok(mut ch) => {
            if ops::write_line(&mut stream, &Reply::Started).await.is_ok() {
                let _ = tokio::io::copy_bidirectional(&mut stream, &mut ch).await;
            }
        }
        Err(reason) => {
            let _ = ops::write_line(&mut stream, &Reply::Refused { reason }).await;
        }
    }
    let _ = stream.shutdown().await;
}

/// The helper: `args` are `<box> <host> <command…>` as rsync runs its remote shell. Its exit
/// code, for the process to exit with at once, since stdin's reader may still be blocked.
pub async fn rsh(args: &[String]) -> i32 {
    match bridge(args).await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("moor-machine-runner rsh: {e}");
            1
        }
    }
}

async fn bridge(args: &[String]) -> Result<(), String> {
    let [box_id, _host, command @ ..] = args else {
        return Err("usage: moor-machine-runner rsh <box> <host> <command>…".to_owned());
    };
    let box_id = box_id
        .parse()
        .map_err(|_| format!("{box_id:?} is not a Box id"))?;
    let path = std::env::var_os(SOCKET_ENV).ok_or_else(|| format!("{SOCKET_ENV} is not set"))?;
    let mut stream = UnixStream::connect(&path)
        .await
        .map_err(|e| format!("cannot reach the runner: {e}"))?;
    let ask = Ask {
        box_id,
        argv: command.to_vec(),
    };
    ops::write_line(&mut stream, &ask)
        .await
        .map_err(|e| e.to_string())?;
    match ops::read_line(&mut stream).await? {
        Reply::Started => {}
        Reply::Refused { reason } => return Err(reason),
        _ => return Err("the runner answered no sync".to_owned()),
    }
    let (mut rd, mut wr) = stream.into_split();
    tokio::spawn(async move {
        let _ = tokio::io::copy(&mut tokio::io::stdin(), &mut wr).await;
        let _ = wr.shutdown().await;
    });
    let mut stdout = tokio::io::stdout();
    tokio::io::copy(&mut rd, &mut stdout)
        .await
        .map_err(|e| e.to_string())?;
    stdout.flush().await.map_err(|e| e.to_string())
}
