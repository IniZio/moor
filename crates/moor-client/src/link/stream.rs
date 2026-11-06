//! The streams on a link: yamux driven over the socket, each stream counted while it is open.

use std::future::Future;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt};

use super::Socket;

/// One stream on the link. The link counts it open until it is dropped.
pub struct Channel {
    /// The stream.
    stream: Compat<yamux::Stream>,
    /// Held while the stream is open; `drive` waits for every one to drop while it drains.
    _open: mpsc::Sender<()>,
}

impl AsyncRead for Channel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for Channel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}
/// Where a stream asked of the link is sent once it opens.
pub type Asked = oneshot::Sender<Result<Channel, String>>;

/// What the link did next.
enum Event {
    /// A stream asked for opened, or could not.
    Opened(Result<yamux::Stream, yamux::ConnectionError>),
    /// The far end opened a stream, the link failed, or it closed.
    Inbound(Option<Result<yamux::Stream, yamux::ConnectionError>>),
    /// The link drains and no stream is open any more.
    Drained,
}

/// Runs yamux on `socket` until the link ends; why it ended.
///
/// It opens a stream for each ask on `asks` and hands each stream the far end opens to `take`.
/// Once `drain` resolves, the link ends as soon as no stream is open.
pub async fn drive(
    socket: Socket,
    mode: yamux::Mode,
    mut asks: mpsc::Receiver<Asked>,
    mut take: impl FnMut(Channel),
    drain: impl Future<Output = ()>,
) -> String {
    let mut conn = yamux::Connection::new(socket, yamux::Config::default(), mode);
    let mut asking: Option<Asked> = None;
    let mut drain = std::pin::pin!(drain);
    let (open, mut closed) = mpsc::channel::<()>(1);
    let counted = open.downgrade();
    let mut open = Some(open);
    let channel = |stream: yamux::Stream| {
        counted.upgrade().map(|open| Channel {
            stream: stream.compat(),
            _open: open,
        })
    };
    loop {
        let event = std::future::poll_fn(|cx| {
            if open.is_some() && drain.as_mut().poll(cx).is_ready() {
                open = None;
            }
            if open.is_none() && closed.poll_recv(cx).is_ready() {
                return Poll::Ready(Event::Drained);
            }
            if asking.is_none() {
                if let Poll::Ready(Some(ask)) = asks.poll_recv(cx) {
                    asking = Some(ask);
                }
            }
            if asking.is_some() {
                if let Poll::Ready(opened) = conn.poll_new_outbound(cx) {
                    return Poll::Ready(Event::Opened(opened));
                }
            }
            conn.poll_next_inbound(cx).map(Event::Inbound)
        })
        .await;
        match event {
            Event::Opened(opened) => {
                if let Some(ask) = asking.take() {
                    let _ = ask.send(
                        opened
                            .map_err(|e| format!("the link cannot open a stream: {e}"))
                            .and_then(|s| channel(s).ok_or_else(|| "the link drained".to_owned())),
                    );
                }
            }
            Event::Inbound(Some(Ok(stream))) => {
                if let Some(ch) = channel(stream) {
                    take(ch);
                }
            }
            Event::Inbound(Some(Err(e))) => return format!("the link failed: {e}"),
            Event::Inbound(None) => return "the link closed".to_owned(),
            Event::Drained => {
                // Dropping the connection would lose the last streams' unsent frames.
                let _ = std::future::poll_fn(|cx| conn.poll_close(cx)).await;
                return "the link drained".to_owned();
            }
        }
    }
}
