//! The links broker: the controller, and ssh through `moor link-dial`, reach a pool host's
//! leases over the link this runner holds, through a socket only this user may open.

use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use moor_client::link::broker::{Ask, Brokered, PoolOp, SOCKET};
use moor_client::link::ops::{self, Op, Reply};
use tokio::net::{UnixListener, UnixStream};

use crate::Agents;

/// Listens on [`SOCKET`] beside the runner's `socket` to the controller, mode 0600; the
/// listener and this user's uid.
///
/// # Errors
/// A socket left there is open to other users, or it cannot be bound.
pub fn bind_broker(socket: &Path) -> Result<(UnixListener, u32), String> {
    let path = socket.with_file_name(SOCKET);
    let failed = |e: io::Error| format!("cannot listen on {}: {e}", path.display());
    let loose = |mode: u32| {
        format!(
            "{} is open to other users (mode {:o}); make it private or remove it",
            path.display(),
            mode & 0o777
        )
    };
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.mode() & 0o077 != 0 => return Err(loose(meta.mode())),
        Ok(_) => std::fs::remove_file(&path).map_err(failed)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(failed(e)),
    }
    let listener = UnixListener::bind(&path).map_err(failed)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(failed)?;
    let uid = std::fs::metadata(&path).map_err(failed)?.uid();
    Ok((listener, uid))
}

/// Carries asks on `listener` from processes of user `uid` until accepting fails.
///
/// # Errors
/// Returns the error that stopped the listener.
pub async fn serve_broker(listener: UnixListener, agents: Agents, uid: u32) -> io::Result<()> {
    loop {
        let (conn, _) = listener.accept().await?;
        if conn.peer_cred().is_ok_and(|c| c.uid() == uid) {
            let agents = agents.clone();
            tokio::spawn(async move { carry(conn, &agents).await });
        }
    }
}

/// Serves one ask: the agent's reply, then for a started dial the bytes both ways until
/// either side closes.
async fn carry(mut conn: UnixStream, agents: &Agents) {
    let (answer, stream) = match ops::read_line::<_, Ask>(&mut conn).await {
        Ok(ask) => relay(agents, ask).await,
        Err(e) => (
            Brokered::Refused {
                reason: format!("the broker carries one lease or dial: {e}"),
            },
            None,
        ),
    };
    if ops::write_line(&mut conn, &answer).await.is_err() {
        return;
    }
    if let Some(mut stream) = stream {
        let _ = tokio::io::copy_bidirectional(&mut conn, &mut stream).await;
    }
}

/// Sends `ask` on its machine's link; the answer, and the stream when a dial started.
async fn relay(agents: &Agents, ask: Ask) -> (Brokered, Option<moor_client::link::Channel>) {
    let machine_id = ask.machine_id;
    let dial = matches!(ask.op, PoolOp::Dial { .. });
    let offline = |reason: String| (Brokered::Offline { machine_id, reason }, None);
    let mut stream = match agents.open(machine_id).await {
        Ok(stream) => stream,
        Err(reason) => return offline(reason),
    };
    if let Err(e) = ops::write_line(&mut stream, &Op::from(ask.op)).await {
        return offline(format!("its link closed: {e}"));
    }
    match ops::read_line::<_, Reply>(&mut stream).await {
        Ok(reply) => {
            let started = dial && reply == Reply::Started;
            (Brokered::Agent { reply }, started.then_some(stream))
        }
        Err(e) => offline(format!("its link closed: {e}")),
    }
}
