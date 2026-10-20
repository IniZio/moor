//! Relays controller events to a browser as a Server-Sent Events stream.

use std::io::{self, Write};
use std::net::Shutdown;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use moor_core::controller::WatchFilter;

use crate::db::Db;
use crate::http::{placeholder_handler, Handler, HttpRequest, Reply};
use crate::sse;
use moor_client::{Client, WatchItem};

/// Timing and buffering of a relay.
#[derive(Clone, Copy, Debug)]
pub struct RelayConfig {
    /// Quiet time after which a comment line goes out, which also notices a closed browser.
    pub keepalive: Duration,
    /// First wait before re-dialling the controller.
    pub backoff_min: Duration,
    /// Longest wait before re-dialling the controller.
    pub backoff_max: Duration,
    /// Items the reader thread may queue ahead of the browser.
    pub channel_bound: usize,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            keepalive: Duration::from_secs(15),
            backoff_min: Duration::from_millis(250),
            backoff_max: Duration::from_secs(10),
            channel_bound: 64,
        }
    }
}

/// State of one browser stream across controller connections.
struct Relay<'a> {
    /// The browser side.
    out: &'a mut dyn Write,
    /// Where delivered cursors are recorded.
    db: &'a Mutex<Db>,
    /// Timing and buffering.
    config: RelayConfig,
    /// Cursor of the last event delivered; the next dial resumes after it.
    cursor: i64,
    /// Wait before the next re-dial.
    backoff: Duration,
}

impl Relay<'_> {
    /// Dials the controller and pumps one connection; `Some` is why it ended, `None` is EOF.
    fn connection(&mut self, client: &Client) -> io::Result<Option<String>> {
        let watch = match client.watch(self.cursor, WatchFilter::default(), true) {
            Ok(watch) => watch,
            Err(e) => return Ok(Some(e.to_string())),
        };
        let handle = match watch.shutdown_handle() {
            Ok(handle) => handle,
            Err(e) => return Ok(Some(e.to_string())),
        };
        let (tx, rx) = mpsc::sync_channel(self.config.channel_bound);
        let reader = thread::spawn(move || {
            for item in watch {
                if tx.send(item).is_err() {
                    break;
                }
            }
        });
        let ended = self.pump(&rx);
        drop(rx);
        let _ = handle.shutdown(Shutdown::Both);
        let _ = reader.join();
        ended
    }

    /// Forwards items from `rx` until the connection ends or the browser write fails.
    fn pump(
        &mut self,
        rx: &Receiver<Result<WatchItem, moor_client::ClientError>>,
    ) -> io::Result<Option<String>> {
        loop {
            match rx.recv_timeout(self.config.keepalive) {
                Ok(Ok(WatchItem::Event(record))) => {
                    send(self.out, sse::event_frame(&record).as_bytes())?;
                    self.cursor = record.cursor;
                    self.backoff = self.config.backoff_min;
                    // A failed save must not stop live delivery; the next event saves again.
                    let _ = self
                        .db
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .set_cursor(record.cursor);
                }
                Ok(Ok(WatchItem::CaughtUp { .. })) => {}
                Ok(Err(e)) => return Ok(Some(e.to_string())),
                Err(RecvTimeoutError::Timeout) => send(self.out, sse::KEEPALIVE)?,
                Err(RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }

    /// Sleeps out the backoff, writing a keepalive per slice so a gone browser is noticed.
    fn wait(&mut self) -> io::Result<()> {
        let mut left = self.backoff;
        while !left.is_zero() {
            let slice = left.min(self.config.keepalive);
            thread::sleep(slice);
            left -= slice;
            send(self.out, sse::KEEPALIVE)?;
        }
        self.backoff = (self.backoff * 2).min(self.config.backoff_max);
        Ok(())
    }
}

/// Writes the controller's events from `since` on to `out`, re-dialling after any failure.
///
/// Each delivered cursor goes to `db`. Ends only when a write to `out` fails; the controller
/// connection is closed on the way out.
///
/// # Errors
/// Returns the write error of the browser connection.
pub fn relay(
    client: &Client,
    since: i64,
    out: &mut dyn Write,
    db: &Mutex<Db>,
    config: RelayConfig,
) -> io::Result<()> {
    let mut relay = Relay {
        out,
        db,
        config,
        cursor: since,
        backoff: config.backoff_min,
    };
    loop {
        if let Some(message) = relay.connection(client)? {
            send(relay.out, sse::error_frame(&message).as_bytes())?;
        }
        relay.wait()?;
    }
}

/// Writes `bytes` and flushes.
fn send(out: &mut dyn Write, bytes: &[u8]) -> io::Result<()> {
    out.write_all(bytes)?;
    out.flush()
}

/// Serves `GET /events` from the controller; every other request gets the placeholder reply.
///
/// A request without `Last-Event-ID` or `since` streams from the start, not from `db`'s cursor.
#[must_use]
pub fn handler(client: Client, db: Arc<Mutex<Db>>, config: RelayConfig) -> Handler {
    let fallback = placeholder_handler();
    Arc::new(move |req: &HttpRequest| {
        if req.method != "GET" || req.path != "/events" {
            return fallback(req);
        }
        let since = sse::start_cursor(req.header("last-event-id"), req.query_param("since"));
        let client = client.clone();
        let db = Arc::clone(&db);
        Reply::Stream(Box::new(move |out| relay(&client, since, out, &db, config)))
    })
}

#[cfg(test)]
mod tests;
