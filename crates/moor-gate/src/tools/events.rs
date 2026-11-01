//! `list_events`: a read of the controller's event log from an offset the caller keeps. The
//! gate holds no state per caller: each call is one watch from `after`, ended once it has what
//! it will return.

use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use moor_client::{ClientError, EventRecord, WatchItem};
use moor_core::controller::WatchFilter;
use moor_core::name::BoxName;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::listen::enrolled;
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
struct EventsArgs {
    /// Return events with an offset greater than this; 0 (the start of the log) by default.
    after: Option<i64>,
    /// Only these event kinds.
    kinds: Option<Vec<String>>,
    /// Only this Box's events, by name.
    r#box: Option<String>,
    /// Most events to return: 50 by default, 100 at most.
    limit: Option<usize>,
    /// When nothing is past `after`, wait up to this many seconds for an event: 0 by default,
    /// 30 at most.
    wait_seconds: Option<u64>,
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
        let socket = self.front_socket.clone();
        let read = tokio::task::spawn_blocking(move || fetch(&socket, &auth.person, args)).await;
        match read {
            Ok(Ok(value)) => CallToolResult::structured(value),
            Ok(Err(why)) => failed(why),
            Err(e) => failed(format!("the call did not finish: {e}")),
        }
    }
}

/// One watch of `person`'s events from `args.after`, as the JSON `list_events` returns.
fn fetch(socket: &Path, person: &Person, args: EventsArgs) -> Result<Value, String> {
    let after = args.after.unwrap_or(0).max(0);
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let wait = Duration::from_secs(args.wait_seconds.unwrap_or(0).min(MAX_WAIT));
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
    let client = enrolled(socket, person)?;
    let watch = client
        .watch(after, filter, !wait.is_zero())
        .map_err(|e| format!("the controller is unreachable: {e}"))?;
    let stop = watch.stopper();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for item in watch {
            if tx.send(item).is_err() {
                return;
            }
        }
    });
    let read = collect(&rx, after, limit, wait);
    stop.stop();
    let (events, cursor) = read?;
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

/// Reads `rx` until `limit` events are past `after`, or the backlog is done and, when `wait`
/// is not zero, an event came or the wait ended. Gives the events and the caught-up cursor,
/// when the controller said caught up.
fn collect(
    rx: &mpsc::Receiver<Result<WatchItem, ClientError>>,
    after: i64,
    limit: usize,
    wait: Duration,
) -> Result<(Vec<Event>, Option<i64>), String> {
    let mut events = Vec::new();
    let mut caught_up = None;
    loop {
        let item = match caught_up {
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(_) if !events.is_empty() => break,
            Some(_) => rx.recv_timeout(wait),
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
            Ok(Ok(WatchItem::CaughtUp { cursor })) => {
                caught_up = Some(cursor);
                if wait.is_zero() {
                    break;
                }
            }
            Ok(Err(e)) => return Err(format!("the controller ended the watch: {e}")),
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => {
                return Err("the controller ended the watch".to_owned())
            }
        }
    }
    Ok((events, caught_up))
}
