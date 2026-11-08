//! `list_events`: a read of the controller's event log from an offset the caller keeps. The
//! gate holds no state per caller: each call is one watch from `after`, ended once it has what
//! it will return.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use moor_client::{Client, ClientError, EventRecord, WatchItem};
use moor_core::controller::WatchFilter;
use moor_core::name::BoxName;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::listen::{enrolled, Unreachable};
use super::{auth_for, failed, Tools};
use crate::mcp::READ;
use crate::Person;

/// Events returned when `limit` is left out.
const DEFAULT_LIMIT: usize = 50;

/// Most events one call returns.
const MAX_LIMIT: usize = 100;

/// Longest `wait_seconds`.
const MAX_WAIT: u64 = 30;

/// Arguments of `list_events`.
#[derive(Deserialize, JsonSchema)]
pub(crate) struct EventsArgs {
    /// Return events with an offset greater than this; 0 (the start of the log) by default.
    pub(crate) after: Option<i64>,
    /// Only these event kinds.
    pub(crate) kinds: Option<Vec<String>>,
    /// Only this Box's events, by name.
    pub(crate) r#box: Option<String>,
    /// Most events to return: 50 by default, 100 at most.
    pub(crate) limit: Option<usize>,
    /// When nothing is past `after`, wait up to this many seconds for an event: 0 by default,
    /// 30 at most.
    pub(crate) wait_seconds: Option<u64>,
}

#[tool_router(router = events_router, vis = "pub(super)")]
impl Tools {
    /// Reads Box events from an offset.
    #[tool(
        description = "Read your Box events from an offset you keep, for polling or for \
                       catching up after a missed webhook. Pass `after` (the `next` of your \
                       last call, 0 at first); reading marks nothing, so the same call gives \
                       the same events. Needs the moor:read scope."
    )]
    async fn list_events(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<EventsArgs>,
    ) -> CallToolResult {
        let auth = match auth_for(&context, READ) {
            Ok(auth) => auth,
            Err(why) => return failed(why),
        };
        let controller = self.controller.clone();
        let read =
            tokio::task::spawn_blocking(move || fetch(&controller, &auth.person, args)).await;
        match read {
            Ok(Ok(value)) => CallToolResult::structured(value),
            Ok(Err(why)) => failed(why.why),
            Err(e) => failed(format!("the call did not finish: {e}")),
        }
    }
}

/// One watch of `person`'s events from `args.after`, as the JSON `list_events` returns.
pub(crate) fn fetch(
    controller: &Client,
    person: &Person,
    args: EventsArgs,
) -> Result<Value, Unreachable> {
    let after = args.after.unwrap_or(0).max(0);
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let wait = Duration::from_secs(args.wait_seconds.unwrap_or(0).min(MAX_WAIT));
    let until = now() + wait;
    let boxes = args
        .r#box
        .map(|name| BoxName::parse_allow_bare(&name).map(|n| vec![n]))
        .transpose()
        .map_err(|e| e.to_string())?;
    let filter = WatchFilter {
        boxes,
        kinds: args.kinds,
        parent: None,
    };
    let client = enrolled(controller, person)?;
    let (mut events, cursor) = read(&client, after, &filter, limit, None)?;
    if events.is_empty() && !wait.is_zero() {
        let from = after.max(cursor.unwrap_or(after));
        events = read(&client, from, &filter, limit, Some(until))?.0;
    }
    let next = events
        .last()
        .map_or_else(|| after.max(cursor.unwrap_or(after)), |e| e.offset);
    Ok(json!({
        "events": events.iter().map(Event::json).collect::<Vec<_>>(),
        "next": next,
        "caught_up": cursor.is_some(),
    }))
}

/// One event of the log.
struct Event {
    /// Its id in the log.
    offset: i64,
    /// The record as the tool shows it.
    record: EventRecord,
}

impl Event {
    /// The event as `list_events` shows it.
    fn json(&self) -> Value {
        let r = &self.record;
        json!({
            "offset": self.offset,
            "kind": r.kind,
            "box": r.box_name,
            "box_id": r.box_id.map(moor_core::BoxId::get),
            "session": r.session,
            "ts": r.ts,
            "data": r.payload,
        })
    }
}

/// One watch from `after`: with no `until`, the backlog to the controller's caught-up frame;
/// with one, the first events that arrive before it, as a follow never sends caught-up.
fn read(
    client: &Client,
    after: i64,
    filter: &WatchFilter,
    limit: usize,
    until: Option<Instant>,
) -> Result<(Vec<Event>, Option<i64>), Unreachable> {
    let watch = client
        .watch(after, filter.clone(), until.is_some())
        .map_err(|e| Unreachable::from_client(&e))?;
    let stop = watch.stopper();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for item in watch {
            if tx.send(item).is_err() {
                return;
            }
        }
    });
    let read = collect(&rx, after, limit, until);
    stop.stop();
    Ok(read?)
}

/// Reads `rx` until `limit` events are past `after`, the backlog is done, or, with `until`,
/// an event came or the time is up. Gives the events and the caught-up cursor, when the
/// controller said caught up.
fn collect(
    rx: &mpsc::Receiver<Result<WatchItem, ClientError>>,
    after: i64,
    limit: usize,
    until: Option<Instant>,
) -> Result<(Vec<Event>, Option<i64>), String> {
    let mut events = Vec::new();
    loop {
        let item = match until {
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(_) if !events.is_empty() => break,
            Some(t) => rx.recv_timeout(t.saturating_duration_since(now())),
        };
        match item {
            Ok(Ok(WatchItem::Event(record))) if record.cursor > after => {
                events.push(Event {
                    offset: record.cursor,
                    record,
                });
                if events.len() >= limit {
                    break;
                }
            }
            Ok(Ok(WatchItem::Event(_))) => {}
            Ok(Ok(WatchItem::CaughtUp { cursor })) => return Ok((events, Some(cursor))),
            Ok(Err(e)) => return Err(format!("the controller ended the watch: {e}")),
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => {
                return Err("the controller ended the watch".to_owned())
            }
        }
    }
    Ok((events, None))
}

/// The monotonic time a wait is measured against.
#[expect(
    clippy::disallowed_methods,
    reason = "bounds a long poll; no time is stored or shown"
)]
fn now() -> Instant {
    Instant::now()
}
