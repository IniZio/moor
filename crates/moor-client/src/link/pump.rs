//! The pump: carries a link's yamux bytes over its WebSocket, beside the frames, and ends the
//! link once the far end goes silent.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, WriteHalf};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{Bytes, Message};
use tokio_tungstenite::WebSocketStream;
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

use super::Frame;

/// The socket yamux runs on.
pub type Socket = Compat<DuplexStream>;

/// A link that is up: the socket yamux runs on, and the frames sent beside it.
pub struct Carried {
    /// The end yamux runs on.
    pub socket: Socket,
    /// Frames to send the far end.
    pub tell: mpsc::Sender<Frame>,
    /// Frames the far end sent.
    pub heard: mpsc::Receiver<Frame>,
}

/// Spawns a pump that carries yamux's bytes over `ws` and pings every `keepalive`.
///
/// The pump closes the socket when `ws` closes or fails, or nothing arrives for three
/// `keepalive` periods. It tells `say` when the far end has been silent for two, and when it
/// closes for silence.
pub fn carry<S>(
    ws: WebSocketStream<S>,
    keepalive: Duration,
    say: impl Fn(&str) + Send + 'static,
) -> Carried
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (near, far) = tokio::io::duplex(64 * 1024);
    let (tell, told) = mpsc::channel(4);
    let (hear, heard) = mpsc::channel(4);
    tokio::spawn(pump(ws, far, (keepalive, say), (told, hear)));
    Carried {
        socket: near.compat(),
        tell,
        heard,
    }
}

/// Moves bytes between `ws` and `far` until either ends or the far end goes quiet, sending the
/// frames told and passing on the frames heard.
///
/// Reading `ws` never waits on yamux: yamux stops reading while it holds an answer it cannot
/// write, so a pump that waited on yamux to take bytes before reading its writes again would
/// stall both for good. What it reads queues for yamux instead, and yamux's flow control
/// bounds the queue.
async fn pump<S>(
    ws: WebSocketStream<S>,
    far: DuplexStream,
    (keepalive, say): (Duration, impl Fn(&str) + Send),
    (mut told, hear): (mpsc::Receiver<Frame>, mpsc::Sender<Frame>),
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sink, mut stream) = ws.split();
    let (mut from_yamux, to_yamux) = tokio::io::split(far);
    let (queue, queued) = mpsc::unbounded_channel();
    let feeding = tokio::spawn(feed(queued, to_yamux));
    let heard = Mutex::new(Instant::now());
    let reading = async {
        while let Some(msg) = stream.next().await {
            *heard.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
            if !deliver(msg, &queue, &hear) {
                break;
            }
        }
    };
    let writing = async {
        let mut tick = tokio::time::interval_at(Instant::now() + keepalive, keepalive);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let sent = tokio::select! {
                n = from_yamux.read(&mut buf) => {
                    let n = n.map_or(0, |n| ready(&mut from_yamux, &mut buf, n));
                    match buf.get(..n) {
                        Some(bytes) if !bytes.is_empty() => sink.send(Message::binary(bytes.to_vec())).await,
                        _ => break,
                    }
                }
                Some(frame) = told.recv() => match serde_json::to_string(&frame) {
                    Ok(text) => sink.send(Message::text(text)).await,
                    Err(_) => continue,
                },
                _ = tick.tick() => sink.send(Message::Ping(Vec::new().into())).await,
            };
            if sent.is_err() {
                break;
            }
        }
    };
    tokio::select! {
        () = reading => {}
        () = writing => {}
        () = quiet(&heard, keepalive, say) => {}
    }
    drop(queue);
    // A link gone quiet may never take the close frame.
    let _ = tokio::time::timeout(keepalive, sink.close()).await;
    // yamux answers the last frames it reads, such as window updates; a dropped pipe would fail
    // those writes, and yamux would end the link before its streams read the data.
    let mut drained = tokio::io::sink();
    let _ = tokio::join!(feeding, tokio::io::copy(&mut from_yamux, &mut drained));
}

/// Resolves once nothing has been `heard` for three `keepalive` periods, telling `say` after
/// two and again on giving up.
async fn quiet(heard: &Mutex<Instant>, keepalive: Duration, say: impl Fn(&str)) {
    let (warn_after, dead_after) = (keepalive * 2, keepalive * 3);
    let mut warned = false;
    loop {
        let last = *heard.lock().unwrap_or_else(PoisonError::into_inner);
        let silent = last.elapsed();
        if silent >= dead_after {
            let secs = dead_after.as_secs_f32();
            say(&format!(
                "heard nothing on the link for {secs}s; closing it"
            ));
            return;
        }
        if silent >= warn_after && !warned {
            let secs = warn_after.as_secs_f32();
            say(&format!("heard nothing on the link for {secs}s"));
        }
        warned = silent >= warn_after;
        tokio::time::sleep_until(last + if warned { dead_after } else { warn_after }).await;
    }
}

/// Adds to the `n` bytes in `buf` what yamux has already written, so a frame's header and body
/// leave in one message.
fn ready(from: &mut tokio::io::ReadHalf<DuplexStream>, buf: &mut [u8], mut n: usize) -> usize {
    use futures_util::FutureExt;
    while n > 0 && n < buf.len() {
        match buf
            .get_mut(n..)
            .and_then(|rest| from.read(rest).now_or_never())
        {
            Some(Ok(more)) if more > 0 => n += more,
            _ => break,
        }
    }
    n
}

/// Writes each queued message's bytes to yamux, then ends yamux's input.
async fn feed(mut queued: mpsc::UnboundedReceiver<Bytes>, mut to_yamux: WriteHalf<DuplexStream>) {
    while let Some(bytes) = queued.recv().await {
        if to_yamux.write_all(&bytes).await.is_err() {
            break;
        }
    }
    let _ = to_yamux.shutdown().await;
}

/// Queues a binary message's bytes for yamux and passes a frame to `hear`; false once the link
/// is over. A text message that is no frame this release knows is skipped.
fn deliver(
    msg: Result<Message, tokio_tungstenite::tungstenite::Error>,
    queue: &mpsc::UnboundedSender<Bytes>,
    hear: &mpsc::Sender<Frame>,
) -> bool {
    match msg {
        Ok(Message::Binary(bytes)) => queue.send(bytes).is_ok(),
        Ok(Message::Text(text)) => {
            if let Ok(frame) = serde_json::from_str(&text) {
                let _ = hear.try_send(frame);
            }
            true
        }
        Ok(Message::Close(_)) | Err(_) => false,
        Ok(_) => true,
    }
}
