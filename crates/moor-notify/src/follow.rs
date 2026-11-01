//! One hook: enroll its user, follow that user's events on the controller, and post each
//! one of the hook's kinds.
//!
//! A reader thread drains the watch into a channel so a slow receiver never makes the
//! controller drop the watch as a slow client; the poster takes events off the channel in
//! cursor order and saves each cursor once it is done with that event.

use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_client::{Client, EventRecord, WatchItem};
use moor_core::controller::WatchFilter;
use moor_core::request::{EnrollReq, Request, ShowReq};
use moor_core::response::{ExitClass, Response};
use moor_core::user::Identity;
use serde_json::Value;

use crate::config::Hook;
use crate::cursors::Cursors;
use crate::deliver::{with_retries, Endpoint, Retry};
use crate::payload;
use crate::Log;

/// The identity namespace the front asserts; the controller's `fronts` key for its socket.
pub const NAMESPACE: &str = "notify";

/// First wait before dialling the controller again.
const BACKOFF_MIN: Duration = Duration::from_millis(250);

/// Longest wait between dials.
const BACKOFF_MAX: Duration = Duration::from_secs(10);

/// What every hook shares.
#[derive(Clone)]
pub struct Shared {
    /// Where each hook's cursor is kept.
    pub cursors: Arc<Cursors>,
    /// How each event is retried.
    pub retry: Retry,
    /// The front's log.
    pub log: Log,
}

/// The identity the front asserts for the user with `hook`'s email.
///
/// # Errors
/// The email does not make a valid identity.
pub fn identity(hook: &Hook) -> Result<Identity, String> {
    Identity::parse(&format!("{NAMESPACE}:{}", hook.email))
}

/// Follows `hook` through `client`, which acts as the hook's identity, until the process
/// ends.
pub fn run(client: &Client, hook: &Hook, shared: &Shared) {
    let email = hook.email.as_str().to_owned();
    let log = |msg: &str| (shared.log)(&format!("{email}: {msg}"));
    let filter = WatchFilter {
        boxes: None,
        kinds: Some(hook.events.clone()),
        parent: None,
    };
    let since = retrying(&log, || {
        enroll(client, hook)?;
        shared
            .cursors
            .get(&email)
            .map_or_else(|| tip(client, &filter), Ok)
    });
    save(&shared.cursors, &email, since, &log);
    let endpoint = Endpoint::new(hook.url.clone(), hook.secret.clone());
    log(&format!(
        "posting {} to {} from cursor {since}",
        hook.events.join(", "),
        endpoint.shown()
    ));
    let (tx, rx) = mpsc::channel();
    let reader = client.clone();
    let reader_log = Arc::clone(&shared.log);
    let reader_email = email.clone();
    thread::spawn(move || {
        let log = |msg: &str| reader_log(&format!("{reader_email}: {msg}"));
        read(&reader, &filter, since, &tx, &log);
    });
    let mut done = since;
    for event in rx {
        if event.cursor <= done {
            continue;
        }
        post(client, &endpoint, &event, shared.retry, &log);
        done = event.cursor;
        save(&shared.cursors, &email, done, &log);
    }
}

/// Saves `cursor` for `email`, logging a failure; the cursor still holds in memory.
fn save(cursors: &Cursors, email: &str, cursor: i64, log: &dyn Fn(&str)) {
    if let Err(why) = cursors.set(email, cursor) {
        log(&format!("cannot save the cursor: {why}"));
    }
}

/// Posts `event`, with the Box's `show` folded in, and logs how it went.
fn post(
    client: &Client,
    endpoint: &Endpoint,
    event: &EventRecord,
    retry: Retry,
    log: &dyn Fn(&str),
) {
    let shown = event.box_id.and_then(|id| show(client, id));
    let body = payload::body(event, shown.as_ref()).to_string();
    let what = format!(
        "{} of Box {} (delivery {})",
        event.kind,
        event.box_name.as_deref().unwrap_or("-"),
        event.cursor
    );
    let posted = with_retries(
        retry,
        || endpoint.post(&event.kind, event.cursor, body.as_bytes(), now()),
        &thread::sleep,
    );
    match posted {
        Ok(_) => log(&format!("posted {what}")),
        Err(why) => log(&format!("gave up on {what}: {why}")),
    }
}

/// Follows the watch from `since` forever, sending each event to `tx`, and dials again with
/// backoff after each drop from the last cursor it saw. Returns once nobody reads `tx`.
fn read(
    client: &Client,
    filter: &WatchFilter,
    since: i64,
    tx: &Sender<EventRecord>,
    log: &dyn Fn(&str),
) {
    let mut cursor = since;
    let mut wait = BACKOFF_MIN;
    let mut last_failure = None;
    loop {
        let why = match client.watch(cursor, filter.clone(), true) {
            Err(e) => e.to_string(),
            Ok(watch) => {
                let mut ended = "the controller closed the watch".to_owned();
                for item in watch {
                    match item {
                        Ok(WatchItem::Event(event)) => {
                            cursor = event.cursor;
                            wait = BACKOFF_MIN;
                            last_failure = None;
                            if tx.send(event).is_err() {
                                return;
                            }
                        }
                        Ok(WatchItem::CaughtUp { cursor: up }) => cursor = cursor.max(up),
                        Err(e) => {
                            ended = e.to_string();
                            break;
                        }
                    }
                }
                ended
            }
        };
        if last_failure.as_ref() != Some(&why) {
            log(&format!("controller: {why}"));
            last_failure = Some(why);
        }
        thread::sleep(wait);
        wait = wait.saturating_mul(2).min(BACKOFF_MAX);
    }
}

/// Runs `attempt` until it succeeds, waiting with backoff and logging each new failure.
fn retrying<T>(log: &dyn Fn(&str), mut attempt: impl FnMut() -> Result<T, String>) -> T {
    let mut wait = BACKOFF_MIN;
    let mut last = None;
    loop {
        match attempt() {
            Ok(value) => return value,
            Err(why) => {
                if last.as_ref() != Some(&why) {
                    log(&format!("controller: {why}"));
                    last = Some(why);
                }
                thread::sleep(wait);
                wait = wait.saturating_mul(2).min(BACKOFF_MAX);
            }
        }
    }
}

/// Tells the controller the hook's identity has the hook's email, so it is the same user.
fn enroll(client: &Client, hook: &Hook) -> Result<(), String> {
    ask(
        client,
        Request::Enroll(EnrollReq {
            identity: None,
            email: Some(hook.email.clone()),
        }),
    )
    .map(drop)
}

/// The newest cursor: a hook with no saved cursor starts there, so its first run posts no
/// backlog.
fn tip(client: &Client, filter: &WatchFilter) -> Result<i64, String> {
    let watch = client
        .watch(0, filter.clone(), false)
        .map_err(|e| e.to_string())?;
    for item in watch {
        if let WatchItem::CaughtUp { cursor } = item.map_err(|e| e.to_string())? {
            return Ok(cursor);
        }
    }
    Err("the controller ended the backlog without a caught-up frame".to_owned())
}

/// The controller's `show` of Box `id` with its agent; none when it does not answer.
fn show(client: &Client, id: moor_core::BoxId) -> Option<Value> {
    ask(
        client,
        Request::Show(ShowReq {
            target: Some(id.into()),
            events: None,
            agent: true,
            reports: false,
            artifacts: false,
            pane: false,
        }),
    )
    .ok()
    .flatten()
}

/// Runs `request` and hands back its value, or why it failed.
fn ask(client: &Client, request: Request) -> Result<Option<Value>, String> {
    let response: Response = client.request(request, |_| {}).map_err(|e| e.to_string())?;
    match response.class {
        ExitClass::Ok => Ok(response.value),
        _ => Err(response
            .error
            .map_or_else(|| format!("{} failed", response.cmd), |e| e.message)),
    }
}

/// Now, in unix seconds, for the signed timestamp.
#[expect(
    clippy::disallowed_methods,
    reason = "the signature's timestamp is wall-clock time by definition"
)]
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
