//! The links machine agents dial in on: who each agent is, and which machines are online.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::StreamExt;
use moor_client::link::{self, Asked, Channel, Frame, WebSocketStream};
use moor_client::Client;
use moor_core::request::{MachineLink, MachineLinksReq, Request};
use moor_core::view::user::MachineKeysValue;
use ssh_key::rand_core::{OsRng, RngCore};
use ssh_key::{PublicKey, SshSig};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, watch};

use crate::claim::now;
use crate::forwards::{accepted, Routes};

/// The runner's release, which an agent told to update must become.
const VERSION: &str = match option_env!("MOOR_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
/// How long each handshake frame may take to arrive.
const ANSWER: Duration = Duration::from_secs(10);
/// How long a failed `machine-links` report waits before it is sent again.
const RETRY: Duration = Duration::from_secs(1);

/// The machines whose agents hold a link, which execs, hooks and relays open streams on.
#[derive(Clone)]
pub struct Agents {
    /// The controller's operator socket.
    client: Client,
    /// Each machine's link.
    links: Arc<Links>,
    /// Where connections accepted for forwards go.
    pub(crate) routes: Arc<Routes>,
}

impl std::fmt::Debug for Agents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agents").finish_non_exhaustive()
    }
}

impl Agents {
    /// None linked yet; every change is reported to the controller at `socket`.
    #[must_use]
    pub fn new(socket: &Path) -> Self {
        let client = Client::new(socket);
        let (tx, rx) = watch::channel(Vec::new());
        tokio::spawn(report(client.clone(), rx));
        let links = Arc::new(Links {
            held: Mutex::default(),
            report: tx,
            next: Mutex::new(0),
        });
        Self {
            client,
            links,
            routes: Arc::default(),
        }
    }

    /// A new stream to machine `machine_id`'s agent.
    ///
    /// # Errors
    /// The machine holds no link, or the link cannot open a stream.
    pub async fn open(&self, machine_id: i64) -> Result<Channel, String> {
        let asks = self
            .links
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&machine_id)
            .and_then(|h| h.asks.clone())
            .ok_or_else(|| format!("machine {machine_id} is not linked"))?;
        let (ask, opened) = oneshot::channel();
        let gone = || format!("machine {machine_id}'s link closed");
        asks.send(ask).await.map_err(|_| gone())?;
        opened.await.map_err(|_| gone())?
    }
}

/// Accepts agents' links on `listener` until accepting fails, holding each in `agents`.
///
/// # Errors
/// Returns the error that stopped the listener.
pub async fn serve_agents(listener: TcpListener, agents: Agents) -> io::Result<()> {
    let Agents {
        client,
        links,
        routes,
    } = agents;
    loop {
        let (stream, _) = listener.accept().await?;
        let (client, links, routes) = (client.clone(), Arc::clone(&links), Arc::clone(&routes));
        tokio::spawn(async move { serve_link(stream, &client, &links, routes).await });
    }
}

/// One agent's link, from the upgrade until it drops or a newer link replaces it.
async fn serve_link<S>(stream: S, client: &Client, links: &Links, routes: Arc<Routes>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let Ok(mut ws) = link::accept(stream).await else {
        return;
    };
    let (machine_id, version) = match admit(&mut ws, client).await {
        Ok(admitted) => admitted,
        Err(answer) => {
            if let Some(frame) = answer {
                let _ = link::send(&mut ws, &frame).await;
            }
            let _ = ws.close(None).await;
            return;
        }
    };
    let welcome = Frame::Welcome {
        version: VERSION.to_owned(),
    };
    if link::send(&mut ws, &welcome).await.is_err() {
        return;
    }
    let (generation, replaced, asks) = links.up(machine_id, version);
    eprintln!("machine {machine_id} linked");
    let socket = link::carry(ws, link::KEEPALIVE);
    let why = tokio::select! {
        why = link::drive(socket, link::Mode::Server, asks, |ch| {
            let routes = Arc::clone(&routes);
            tokio::spawn(async move { accepted(ch, machine_id, &routes).await });
        }) => why,
        _ = replaced => "a newer link replaced it".to_owned(),
    };
    links.down(machine_id, generation);
    eprintln!("machine {machine_id} unlinked: {why}");
}

/// The machine whose agent proved itself on `ws` and the agent's release, or the frame to
/// answer before closing.
async fn admit<S>(
    ws: &mut WebSocketStream<S>,
    client: &Client,
) -> Result<(i64, String), Option<Frame>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let first = tokio::time::timeout(ANSWER, ws.next())
        .await
        .ok()
        .flatten()
        .and_then(Result::ok)
        .ok_or(None)?;
    let hello = first
        .to_text()
        .ok()
        .and_then(|text| serde_json::from_str::<Frame>(text).ok());
    let Some(Frame::Hello {
        machine_id,
        protocol: link::PROTOCOL,
        version,
    }) = hello
    else {
        return Err(Some(Frame::UpdateRequired {
            version: VERSION.to_owned(),
        }));
    };
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let nonce = bytes.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    });
    let challenge = Frame::Challenge {
        nonce: nonce.clone(),
    };
    link::send(ws, &challenge).await.map_err(|_| None)?;
    let Ok(Frame::Proof { time, signature }) = link::recv(ws, ANSWER).await else {
        return Err(None);
    };
    let key = key_of(client, machine_id).await.map_err(refused)?;
    SshSig::from_pem(signature)
        .ok()
        .filter(|sig| {
            let signed = link::signed(&nonce, machine_id, time);
            key.verify(link::NAMESPACE, &signed, sig).is_ok()
        })
        .map(|_| (machine_id, version))
        .ok_or_else(|| {
            refused(format!(
                "the proof is not signed by machine {machine_id}'s key"
            ))
        })
}

/// A `refused` frame giving `reason`.
#[expect(
    clippy::unnecessary_wraps,
    reason = "it maps admit's errors, which are optional"
)]
fn refused(reason: String) -> Option<Frame> {
    Some(Frame::Refused { reason })
}

/// The key machine `machine_id` was enrolled with, read from the controller each time so a
/// replaced or removed key stops working at once.
async fn key_of(client: &Client, machine_id: i64) -> Result<PublicKey, String> {
    let client = client.clone();
    let response =
        tokio::task::spawn_blocking(move || client.request(Request::MachineKeys {}, |_| {}))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("the server cannot check keys now: {e}"))?;
    if let Some(error) = response.error {
        return Err(format!(
            "the server cannot check keys now: {}",
            error.message
        ));
    }
    let keys: MachineKeysValue = serde_json::from_value(response.value.unwrap_or_default())
        .map_err(|e| format!("machine-keys answered something else: {e}"))?;
    let enrolled = keys
        .machines
        .into_iter()
        .find(|m| m.machine_id == machine_id)
        .ok_or_else(|| format!("machine {machine_id} is not enrolled"))?;
    PublicKey::from_openssh(&enrolled.pubkey)
        .map_err(|e| format!("machine {machine_id}'s key does not parse: {e}"))
}

/// Every machine seen since the runner started, and the link that holds each online one.
struct Links {
    /// By machine id.
    held: Mutex<BTreeMap<i64, Held>>,
    /// The whole set, which `report` sends to the controller after every change.
    report: watch::Sender<Vec<MachineLink>>,
    /// The generation the next link gets.
    next: Mutex<u64>,
}

/// One machine's link state.
struct Held {
    /// What the controller is told.
    link: MachineLink,
    /// Which link set it, so an older link ending leaves a newer one online.
    generation: u64,
    /// Dropping it ends the link that set it.
    end: Option<oneshot::Sender<()>>,
    /// Asks that link for streams; none once it is down.
    asks: Option<mpsc::Sender<Asked>>,
}

impl Links {
    /// Marks `machine_id` online with agent `version`, ending its previous link; the new link's
    /// generation, what resolves once a newer link replaces it, and the asks for its streams.
    fn up(
        &self,
        machine_id: i64,
        version: String,
    ) -> (u64, oneshot::Receiver<()>, mpsc::Receiver<Asked>) {
        let generation = {
            let mut next = self.next.lock().unwrap_or_else(PoisonError::into_inner);
            *next += 1;
            *next
        };
        let (end, replaced) = oneshot::channel();
        let (ask, asks) = mpsc::channel(16);
        let link = MachineLink {
            machine_id,
            online: true,
            since: now(),
            version: Some(version),
            protocol: Some(link::PROTOCOL),
        };
        self.change(|held| {
            held.insert(
                machine_id,
                Held {
                    link,
                    generation,
                    end: Some(end),
                    asks: Some(ask),
                },
            );
        });
        (generation, replaced, asks)
    }

    /// Marks `machine_id` offline, unless a newer link than `generation` holds it.
    fn down(&self, machine_id: i64, generation: u64) {
        self.change(|held| {
            if let Some(h) = held
                .get_mut(&machine_id)
                .filter(|h| h.generation == generation)
            {
                h.link.online = false;
                h.link.since = now();
                h.end = None;
                h.asks = None;
            }
        });
    }

    /// Applies `edit` and publishes the resulting set.
    fn change(&self, edit: impl FnOnce(&mut BTreeMap<i64, Held>)) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        edit(&mut held);
        self.report
            .send_replace(held.values().map(|h| h.link.clone()).collect());
    }
}

/// Sends the latest set as `machine-links` after every change, again until the controller
/// takes it.
async fn report(client: Client, mut rx: watch::Receiver<Vec<MachineLink>>) {
    while rx.changed().await.is_ok() {
        loop {
            let links = rx.borrow_and_update().clone();
            let client = client.clone();
            let request = Request::MachineLinks(MachineLinksReq { links });
            let sent = tokio::task::spawn_blocking(move || client.request(request, |_| {})).await;
            if matches!(sent, Ok(Ok(ref r)) if r.error.is_none()) {
                break;
            }
            tokio::time::sleep(RETRY).await;
        }
    }
}
