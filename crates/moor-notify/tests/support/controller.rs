//! A scripted controller on a Unix socket: it answers the hello, hands queued deliveries to
//! `webhook-claim` (one per webhook until it is reported) and records every claim and report.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use moor_core::controller::{parse_frame, ClientFrame, ServerFrame};
use moor_core::request::{Request, WebhookOutcome};
use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::{json, Value};

/// One queued delivery.
#[derive(Clone)]
pub(crate) struct Queued {
    /// The delivery's id.
    pub(crate) delivery: i64,
    /// The webhook it is for.
    pub(crate) webhook: i64,
    /// Where to post.
    pub(crate) url: String,
    /// The HMAC key.
    pub(crate) key: String,
    /// The event kind.
    pub(crate) event: String,
    /// The body to post.
    pub(crate) body: Value,
    /// The receiver's own header, as name and value.
    pub(crate) header: Option<(String, String)>,
}

/// What clients sent.
#[derive(Default)]
pub(crate) struct Seen {
    /// The `(limit, lease_ms)` of every claim.
    pub(crate) claims: Vec<(u32, u64)>,
    /// Every report, in order.
    pub(crate) reports: Vec<(i64, WebhookOutcome)>,
    /// Whether any request asserted an identity.
    pub(crate) asserted_identity: bool,
}

/// State shared with the connection threads.
#[derive(Default)]
struct Inner {
    /// What clients sent.
    seen: Seen,
    /// Deliveries not yet claimed, oldest first.
    queue: Vec<Queued>,
    /// Deliveries claimed and not yet reported.
    leased: Vec<Queued>,
    /// When set, `webhook-claim` is refused with this message.
    refuse: Option<String>,
}

/// A running scripted controller.
pub(crate) struct FakeController {
    /// State shared with the connection threads.
    inner: Arc<Mutex<Inner>>,
}

impl FakeController {
    /// Binds `socket` and serves it until the test ends.
    pub(crate) fn serve(socket: &Path) -> Self {
        let listener = UnixListener::bind(socket).unwrap();
        let inner = Arc::new(Mutex::new(Inner::default()));
        let shared = Arc::clone(&inner);
        thread::spawn(move || {
            for stream in listener.incoming().map_while(Result::ok) {
                let inner = Arc::clone(&shared);
                thread::spawn(move || serve(stream, &inner));
            }
        });
        Self { inner }
    }

    /// Queues an `event` delivery for webhook `webhook` at `url`, signed with `key`.
    pub(crate) fn queue(&self, delivery: i64, webhook: i64, url: &str, key: &str, event: &str) {
        self.lock().queue.push(Queued {
            delivery,
            webhook,
            url: url.to_owned(),
            key: key.to_owned(),
            event: event.to_owned(),
            body: json!({"schema": "moor.notify/1", "delivery": delivery, "event": event,
                         "box": "app/fix", "data": {"n": delivery}}),
            header: None,
        });
    }

    /// Gives queued delivery `delivery` the receiver's own header `name: value`.
    pub(crate) fn header(&self, delivery: i64, name: &str, value: &str) {
        for d in &mut self.lock().queue {
            if d.delivery == delivery {
                d.header = Some((name.to_owned(), value.to_owned()));
            }
        }
    }

    /// Refuses every claim with `message`.
    pub(crate) fn refuse_claims(&self, message: &str) {
        self.lock().refuse = Some(message.to_owned());
    }

    /// Reads what clients sent so far.
    pub(crate) fn seen<T>(&self, read: impl FnOnce(&Seen) -> T) -> T {
        read(&self.lock().seen)
    }

    /// Waits until `done` holds for what clients sent; fails the test after 30 s.
    pub(crate) fn wait_until(&self, what: &str, done: impl Fn(&Seen) -> bool) {
        for _ in 0..1500 {
            if self.seen(&done) {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("controller never saw {what}");
    }

    /// Locks the shared state.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }
}

/// Serves one connection until the client leaves.
fn serve(stream: UnixStream, inner: &Mutex<Inner>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let mut line = String::new();
    while reader.read_line(&mut line).unwrap_or(0) > 0 {
        let frame = parse_frame(line.trim_end()).unwrap();
        line.clear();
        let reply = match frame {
            ClientFrame::Hello { protocol } => ServerFrame::Hello { protocol },
            ClientFrame::Request { id, request } => {
                let envelope = answer(*request, &mut inner.lock().unwrap());
                ServerFrame::Response { id, envelope }
            }
            _ => return,
        };
        if writer.write_all(reply.to_line().as_bytes()).is_err() {
            return;
        }
    }
}

/// The answer to `request`, recorded.
fn answer(request: Request, inner: &mut Inner) -> Response {
    match request {
        Request::WebhookClaim(claim) => {
            inner.seen.claims.push((claim.limit, claim.lease_ms));
            if let Some(why) = &inner.refuse {
                return refusal("webhook-claim", why);
            }
            let mut busy: HashSet<i64> = inner.leased.iter().map(|d| d.webhook).collect();
            let mut handed = Vec::new();
            let mut kept = Vec::new();
            for d in std::mem::take(&mut inner.queue) {
                if handed.len() < claim.limit as usize && busy.insert(d.webhook) {
                    handed.push(d);
                } else {
                    kept.push(d);
                }
            }
            inner.queue = kept;
            let deliveries: Vec<Value> = handed
                .iter()
                .map(|d| {
                    let mut one = json!({"delivery": d.delivery, "webhook": d.webhook,
                        "url": d.url, "key": d.key, "event": d.event, "body": d.body});
                    if let (Some((name, value)), Some(out)) = (&d.header, one.as_object_mut()) {
                        out.insert("header".to_owned(), json!({"name": name, "value": value}));
                    }
                    one
                })
                .collect();
            inner.leased.extend(handed);
            Response::ok("webhook-claim", Some(json!({"deliveries": deliveries})))
        }
        Request::WebhookReport(report) => {
            inner.leased.retain(|d| d.delivery != report.delivery);
            inner.seen.reports.push((report.delivery, report.outcome));
            Response::ok("webhook-report", Some(json!({"delivery": report.delivery})))
        }
        Request::As(_) => {
            inner.seen.asserted_identity = true;
            refusal("as", "the front asserts no identity")
        }
        other => panic!("unexpected request {other:?}"),
    }
}

/// A refusal of `cmd` with `message`.
fn refusal(cmd: &str, message: &str) -> Response {
    Response::err(
        cmd,
        ErrorView {
            kind: ErrorKind::Refused,
            message: message.to_owned(),
            hints: Vec::new(),
            context: Vec::new(),
            detail: None,
        },
    )
}
