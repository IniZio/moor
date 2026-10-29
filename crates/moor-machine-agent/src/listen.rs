//! Listens on the machine's loopback for a Box while the runner holds the request's stream
//! open, and carries each accepted connection back over the link on a stream of its own.

use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex, PoisonError};

use moor_client::forward::{Accepted, MAX_PORTS};
use moor_client::link::ops::{self, Reply};
use moor_client::link::{Asked, Channel};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;

/// What every listen on one link shares.
#[derive(Clone)]
pub(crate) struct Link {
    /// Asks the link for a stream back to the runner.
    pub(crate) open: mpsc::Sender<Asked>,
    /// The Boxes listening now, across links.
    pub(crate) boxes: Arc<Boxes>,
}

/// The Boxes with a listen up, at most the machine's slots of them.
pub(crate) struct Boxes {
    /// How many Boxes may hold the machine at once.
    slots: usize,
    /// Each listening Box and how many listens it has up.
    held: Mutex<BTreeMap<i64, usize>>,
}

impl Boxes {
    /// None yet, on a machine of `slots`.
    pub(crate) fn new(slots: u16) -> Arc<Self> {
        Arc::new(Self {
            slots: usize::from(slots),
            held: Mutex::default(),
        })
    }

    /// Counts a listen of `box_id`, unless it would make more Boxes than slots.
    fn take(self: &Arc<Self>, box_id: i64) -> Result<Taken, String> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if !held.contains_key(&box_id) && held.len() >= self.slots {
            return Err(format!(
                "{} Boxes already listen, as many as the machine's slots",
                held.len()
            ));
        }
        *held.entry(box_id).or_default() += 1;
        drop(held);
        Ok(Taken {
            boxes: Arc::clone(self),
            box_id,
        })
    }
}

/// One counted listen; dropping it uncounts it.
struct Taken {
    /// Where it is counted.
    boxes: Arc<Boxes>,
    /// Whose it is.
    box_id: i64,
}

impl Drop for Taken {
    fn drop(&mut self) {
        let mut held = self
            .boxes
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(n) = held.get_mut(&self.box_id) {
            *n -= 1;
            if *n == 0 {
                held.remove(&self.box_id);
            }
        }
    }
}

/// Binds `ports` for `box_id` and serves them until the runner closes `ch` or sends more.
pub(crate) async fn serve(mut ch: Channel, link: &Link, box_id: i64, listen: u64, ports: &[u16]) {
    let (taken, listeners) = match bind(&link.boxes, box_id, ports).await {
        Ok(bound) => bound,
        Err(reason) => {
            let _ = ops::write_line(&mut ch, &Reply::Refused { reason }).await;
            let _ = ch.shutdown().await;
            return;
        }
    };
    if ops::write_line(&mut ch, &Reply::Listening).await.is_err() {
        return;
    }
    let mut accepting = JoinSet::new();
    for (port, listener) in listeners {
        accepting.spawn(accept(listener, link.open.clone(), listen, port));
    }
    let mut probe = [0u8; 1];
    let _ = ch.read(&mut probe).await;
    accepting.abort_all();
    drop(taken);
}

/// A listener on the loopback at each of `ports`, counted for `box_id`.
async fn bind(
    boxes: &Arc<Boxes>,
    box_id: i64,
    ports: &[u16],
) -> Result<(Taken, Vec<(u16, TcpListener)>), String> {
    if box_id <= 0 {
        return Err(format!("{box_id} is not a Box id"));
    }
    let distinct: BTreeSet<u16> = ports.iter().copied().collect();
    if ports.is_empty() || ports.len() > MAX_PORTS || distinct.len() != ports.len() {
        return Err(format!(
            "a listen takes 1 to {MAX_PORTS} distinct ports, not {ports:?}"
        ));
    }
    if distinct.contains(&0) {
        return Err("port 0 is not a port".to_owned());
    }
    let taken = boxes.take(box_id)?;
    let mut listeners = Vec::with_capacity(ports.len());
    for &port in ports {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|e| format!("cannot listen on 127.0.0.1:{port}: {e}"))?;
        listeners.push((port, listener));
    }
    Ok((taken, listeners))
}

/// Accepts on `listener` until aborted, which also ends every connection it carries.
async fn accept(listener: TcpListener, open: mpsc::Sender<Asked>, listen: u64, port: u16) {
    let mut carrying = JoinSet::new();
    while let Ok((tcp, _)) = listener.accept().await {
        while carrying.try_join_next().is_some() {}
        carrying.spawn(carry(tcp, open.clone(), Accepted { listen, port }));
    }
}

/// Opens a stream back to the runner for `accepted` and pipes `tcp` through it.
async fn carry(mut tcp: TcpStream, open: mpsc::Sender<Asked>, accepted: Accepted) {
    let (ask, opened) = oneshot::channel();
    if open.send(ask).await.is_err() {
        return;
    }
    let Ok(Ok(mut stream)) = opened.await else {
        return;
    };
    if ops::write_line(&mut stream, &accepted).await.is_ok() {
        let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
    }
}
