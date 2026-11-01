//! The person's Boxes as MCP resources, and `subscriptions/listen` over the controller's
//! watch: a lifecycle event of a Box sends `notifications/resources/updated` for its URI, and
//! a Box that starts or goes sends `notifications/resources/list_changed`.
//!
//! A notification says only which resource changed; the client reads it for the facts, the
//! same JSON `box_status` returns.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use moor_client::{Client, WatchItem, WatchStop};
use moor_core::controller::WatchFilter;
use moor_core::request::{EnrollReq, Request as ControllerRequest};
use rmcp::model::{
    ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
    ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerConfig,
    SubscriptionFilter,
};
use rmcp::service::{RequestContext, SubscriptionContext, SubscriptionSendError};
use rmcp::{tool_handler, ErrorData, RoleServer, ServerHandler};
use serde_json::Value;
use tokio::sync::mpsc;

use super::{auth_for, send, show_all, status_request, target, Reply, Tools};
use crate::mcp::READ;
use crate::{rewrite, Person};

/// Every Box resource's URI starts with this; the Box name follows.
pub(crate) const PREFIX: &str = "moor://box/";

/// Kinds after which a Box's resource reads differently.
const UPDATED: [&str; 13] = [
    "idled",
    "busy",
    "needs_input",
    "setup_failed",
    "session_failed",
    "launch_timed_out",
    "relaunch_capped",
    "relaunched",
    "box_ready",
    "box_failed",
    "asleep",
    "woke",
    "closed",
];

/// Kinds after which the list of Boxes differs.
const LISTED: [&str; 3] = ["started", "closed", "box_deleted"];

/// The URI of Box `name`.
pub(crate) fn uri(name: &str) -> String {
    format!("{PREFIX}{name}")
}

/// One change the watch saw.
enum Change {
    /// The list of Boxes changed.
    Listed,
    /// The Box of this name changed.
    Updated(String),
}

#[allow(
    clippy::unused_async_trait_impl,
    reason = "the generated handlers are async whether or not they await"
)]
#[tool_handler]
impl ServerHandler for Tools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_resources_subscribe()
                .enable_resources_list_changed()
                .build(),
        )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let person = person(&context)?;
        let value = self.ask(person, show_all(), Reply::Listed).await?;
        let rows = value.get("boxes").and_then(Value::as_array);
        let resources = rows
            .into_iter()
            .flatten()
            .filter_map(|row| {
                let name = row.get("name").and_then(Value::as_str)?;
                let status = row
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Some(
                    Resource::new(uri(name), name)
                        .with_description(status)
                        .with_mime_type("application/json"),
                )
            })
            .collect();
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let person = person(&context)?;
        let name = request
            .uri
            .strip_prefix(PREFIX)
            .ok_or_else(|| ErrorData::invalid_params(format!("{} is no Box", request.uri), None))?;
        let show = target(name)
            .map(status_request)
            .map_err(|why| ErrorData::invalid_params(why, None))?;
        let value = self.ask(person, show, Reply::Detailed).await?;
        let text = ResourceContents::text(value.to_string(), request.uri)
            .with_mime_type("application/json");
        Ok(ReadResourceResult::new(vec![text]).into())
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        let mut accepted = SubscriptionFilter::new();
        accepted.resources_list_changed = requested.resources_list_changed;
        accepted.resource_subscriptions = requested.resource_subscriptions.as_ref().map(|uris| {
            uris.iter()
                .filter(|u| u.starts_with(PREFIX))
                .cloned()
                .collect()
        });
        Some(accepted)
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        let person = person(context.request_context())?;
        let (tx, rx) = mpsc::unbounded_channel();
        let stop = Arc::new(Mutex::new(Stopper::default()));
        let (socket, held) = (self.front_socket.clone(), Arc::clone(&stop));
        let watching = tokio::task::spawn_blocking(move || follow(&socket, &person, &tx, &held));
        let watch_ended = relay(&context, rx).await;
        stop.lock().unwrap_or_else(PoisonError::into_inner).stop();
        if !watch_ended {
            return Ok(());
        }
        let why = watching
            .await
            .unwrap_or_else(|e| Err(format!("the watch did not finish: {e}")))
            .err()
            .unwrap_or_else(|| "the controller ended the watch".to_owned());
        Err(ErrorData::internal_error(why, None))
    }
}

/// Sends each change from `rx` on the subscription until the client leaves (false) or the
/// watch ends (true).
async fn relay(context: &SubscriptionContext, mut rx: mpsc::UnboundedReceiver<Change>) -> bool {
    loop {
        let change = tokio::select! {
            () = context.cancelled() => return false,
            change = rx.recv() => change,
        };
        let sink = context.sink();
        let sent = match change {
            None => return true,
            Some(Change::Listed) => sink.notify_resource_list_changed().await,
            Some(Change::Updated(name)) => sink.notify_resource_updated(uri(&name)).await,
        };
        if matches!(sent, Err(SubscriptionSendError::SubscriptionClosed)) {
            return false;
        }
    }
}

impl Tools {
    /// Runs `request` as `person`, shaped by `reply`, off the async runtime.
    async fn ask(
        &self,
        person: Person,
        request: ControllerRequest,
        reply: Reply,
    ) -> Result<Value, ErrorData> {
        let socket = self.front_socket.clone();
        tokio::task::spawn_blocking(move || send(&socket, &person, request, reply))
            .await
            .map_err(|e| ErrorData::internal_error(format!("the call did not finish: {e}"), None))?
            .map_err(|why| ErrorData::internal_error(why, None))
    }
}

/// The person `context` speaks for, when its token may read.
fn person(context: &RequestContext<RoleServer>) -> Result<Person, ErrorData> {
    auth_for(context, READ)
        .map(|auth| auth.person)
        .map_err(|why| ErrorData::invalid_request(why, None))
}

/// Whether the listen ended, and the live watch to end when it does.
#[derive(Default)]
struct Stopper {
    /// Set once the listen ended.
    stopped: bool,
    /// The watch to end.
    watch: Option<WatchStop>,
}

impl Stopper {
    /// Ends the watch, and any watch handed over later.
    fn stop(&mut self) {
        self.stopped = true;
        if let Some(watch) = self.watch.take() {
            watch.stop();
        }
    }
}

/// Follows `person`'s events from now until the watch ends or `stop` is set, sending each
/// change to `tx`. An `Err` says why the controller ended it.
fn follow(
    socket: &Path,
    person: &Person,
    tx: &mpsc::UnboundedSender<Change>,
    stop: &Mutex<Stopper>,
) -> Result<(), String> {
    let client = enrolled(socket, person)?;
    let mut kinds: Vec<String> = UPDATED.iter().map(|&k| k.to_owned()).collect();
    kinds.extend(
        LISTED
            .iter()
            .filter(|k| !UPDATED.contains(k))
            .map(|&k| k.to_owned()),
    );
    let filter = WatchFilter {
        boxes: None,
        kinds: Some(kinds),
        parent: None,
    };
    let since = tip(&client, &filter)?;
    let watch = client
        .watch(since, filter, true)
        .map_err(|e| format!("the controller is unreachable: {e}"))?;
    {
        let mut held = stop.lock().unwrap_or_else(PoisonError::into_inner);
        if held.stopped {
            return Ok(());
        }
        held.watch = Some(watch.stopper());
    }
    for item in watch {
        let event = match item {
            Ok(WatchItem::Event(event)) => event,
            Ok(WatchItem::CaughtUp { .. }) => continue,
            Err(e) => return Err(format!("the controller ended the watch: {e}")),
        };
        let name = event.box_name.unwrap_or_default();
        let kind = event.kind.as_str();
        if LISTED.contains(&kind) && tx.send(Change::Listed).is_err() {
            return Ok(());
        }
        if UPDATED.contains(&kind) && !name.is_empty() && tx.send(Change::Updated(name)).is_err() {
            return Ok(());
        }
    }
    Ok(())
}

/// A client acting as `person`, once the controller knows their identity and email.
fn enrolled(socket: &Path, person: &Person) -> Result<Client, String> {
    let enroll = ControllerRequest::Enroll(EnrollReq {
        identity: None,
        email: None,
    });
    let client = Client::new(PathBuf::from(socket));
    let reply = client
        .request(rewrite::act(enroll, person)?, |_| {})
        .map_err(|e| format!("the controller is unreachable: {e}"))?;
    super::answer(&reply)?;
    Ok(client.acting_as(person.identity.clone()))
}

/// The newest cursor, so a listen tells only of what happens after it starts.
fn tip(client: &Client, filter: &WatchFilter) -> Result<i64, String> {
    let backlog = client
        .watch(0, filter.clone(), false)
        .map_err(|e| format!("the controller is unreachable: {e}"))?;
    for item in backlog {
        if let WatchItem::CaughtUp { cursor } = item.map_err(|e| e.to_string())? {
            return Ok(cursor);
        }
    }
    Err("the controller ended the backlog without a caught-up frame".to_owned())
}
