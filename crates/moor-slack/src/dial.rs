//! Dialling the controller: the backoff between attempts and the watch reader's loop.

use std::sync::mpsc::Sender;
use std::time::Duration;

use moor_client::{Client, ClientError, Watch, WatchItem};
use moor_core::controller::WatchFilter;

/// First wait before dialling the controller again.
const BACKOFF_MIN: Duration = Duration::from_millis(250);

/// Longest wait between dials.
const BACKOFF_MAX: Duration = Duration::from_secs(10);

/// A wait between dials that doubles up to [`BACKOFF_MAX`].
#[derive(Debug)]
pub(crate) struct Backoff(Duration);

impl Backoff {
    /// Starts at [`BACKOFF_MIN`].
    pub(crate) const fn new() -> Self {
        Self(BACKOFF_MIN)
    }

    /// The wait to take now; the following one doubles.
    pub(crate) fn next(&mut self) -> Duration {
        let wait = self.0;
        self.0 = wait.saturating_mul(2).min(BACKOFF_MAX);
        wait
    }
}

/// Why one watch connection ended early.
enum Ended {
    /// The controller could not be reached or broke the stream.
    Failed(ClientError),
    /// Nobody reads the forwarded items any more.
    FeedGone,
}

/// The watch reader's dial state: the next wait and the last failure it logged.
struct Dialer {
    /// Wait before the next dial.
    backoff: Backoff,
    /// Text of the last failure logged since the last healthy connection.
    logged: Option<String>,
}

impl Dialer {
    /// A connection delivered its backlog: dial again quickly and log the next failure.
    fn healthy(&mut self) {
        self.backoff = Backoff::new();
        self.logged = None;
    }

    /// Logs `e` unless it repeats the last failure logged.
    fn failed(&mut self, e: &ClientError, log: &dyn Fn(&str)) {
        let text = format!("controller: {e}");
        if self.logged.as_ref() != Some(&text) {
            log(&text);
            self.logged = Some(text);
        }
    }
}

/// Forwards the controller's events to `feed` until the feed is gone, dialling again after
/// every drop with backoff.
///
/// The backoff resets only once a backlog is caught up, so a controller that accepts the
/// hello and then fails every watch is not dialled in a tight loop. A failure is logged only
/// when its text differs from the previous one, so a controller that is down or speaks
/// another protocol shows one line per cause, not one per attempt.
pub(crate) fn read_events(
    client: &Client,
    feed: &Sender<WatchItem>,
    log: &dyn Fn(&str),
    sleep: &dyn Fn(Duration),
) {
    let mut dialer = Dialer {
        backoff: Backoff::new(),
        logged: None,
    };
    loop {
        let ended = client
            .watch(0, WatchFilter::default(), false)
            .map_err(Ended::Failed)
            .and_then(|backlog| forward(client, backlog, feed, &mut dialer));
        match ended {
            Ok(()) => {}
            Err(Ended::FeedGone) => return,
            Err(Ended::Failed(e)) => dialer.failed(&e, log),
        }
        sleep(dialer.backoff.next());
    }
}

/// Forwards the backlog, then follows live events from the cursor its caught-up frame names.
/// A backlog that closes without a caught-up frame ends the connection.
fn forward(
    client: &Client,
    backlog: Watch,
    feed: &Sender<WatchItem>,
    dialer: &mut Dialer,
) -> Result<(), Ended> {
    let mut caught_up = None;
    for item in backlog {
        let item = item.map_err(Ended::Failed)?;
        if let WatchItem::CaughtUp { cursor } = item {
            caught_up = Some(cursor);
            dialer.healthy();
        }
        feed.send(item).map_err(|_| Ended::FeedGone)?;
    }
    let Some(cursor) = caught_up else {
        return Ok(());
    };
    let live = client
        .watch(cursor, WatchFilter::default(), true)
        .map_err(Ended::Failed)?;
    for item in live {
        feed.send(item.map_err(Ended::Failed)?)
            .map_err(|_| Ended::FeedGone)?;
    }
    Ok(())
}
