//! Relays: stdio MCP servers on the machine whose tools the runner lists as its own.
//!
//! Per Box and relay the runner keeps one MCP client session to the server, started over ssh
//! in the Box's copy on its first relayed call or tool listing. Each relayed tool is listed as
//! `<relay>_<tool>`. A forced argument is dropped from the tool's schema and set from the Box's
//! facts on every call, as the type the schema gave it; a hidden tool is not listed or callable, so a Box only ever reaches what was set up for it. A session ends when
//! its Box is released, after it sits idle, or when its server exits; the next call starts it
//! again.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, NumberOrString, ProgressNotificationParam,
    ProgressToken, Tool,
};
use rmcp::service::{NotificationContext, Peer, RequestContext, RunningService, ServiceError};
use rmcp::{ClientHandler, ErrorData, RoleClient, RoleServer, ServiceExt};
use tokio::process::{Child, Command};
use tokio::time::Instant;

use crate::caller::Caller;
use crate::exec;
use crate::hooks::{self, Facts};
use crate::relay::tools::{expose, mcp_name, Route};

mod tools;

/// One stdio MCP server on the machine, as the operator named it.
#[derive(Debug, Clone)]
pub struct Relay {
    /// Prefix of its tools' names: `[a-z][a-z0-9-]*`.
    pub name: String,
    /// Command run on the machine, in the Box's copy, through the login shell.
    pub command: String,
    /// Argument name to the fact it is set from on every call, such as `udid` = `device`.
    pub forced: BTreeMap<String, String>,
    /// Names, on the relay, of tools left out of the listing and refused when called.
    pub hidden: BTreeSet<String>,
}

/// Every relay and the sessions open to them.
pub(crate) struct Relays {
    /// The relays, in flag order.
    list: Vec<Relay>,
    /// How long a session may go without a call.
    idle: Duration,
    /// How long starting a session may take.
    start_timeout: Duration,
    /// One slot per Box id and relay index; a slot's lock is held while its session starts.
    sessions: Mutex<HashMap<(i64, usize), Slot>>,
    /// Where relayed progress goes back to.
    progress: Progress,
    /// Source of the progress tokens sent to relays.
    next_token: AtomicU64,
}

/// A session, or none yet, behind the lock that starts it.
type Slot = Arc<tokio::sync::Mutex<Option<Session>>>;

/// The relays' own progress tokens to the caller's token and the peer that asked.
type Progress = Arc<Mutex<HashMap<String, (ProgressToken, Peer<RoleServer>)>>>;

/// An open client session to one relay for one Box.
struct Session {
    /// The MCP client; dropping it closes the server's stdin.
    service: RunningService<RoleClient, Forward>,
    /// The ssh process carrying the session.
    child: Child,
    /// The relay's tools as listed to the Box.
    tools: Vec<Tool>,
    /// Listed name to how a call to it is relayed.
    routes: HashMap<String, Route>,
    /// When the session was last used and how many calls it carries now.
    used: Arc<Used>,
}

/// When a session was last used and how many calls are in it.
struct Used {
    /// End of the last call, or the session's start.
    last: Mutex<Instant>,
    /// Calls in flight.
    calls: AtomicUsize,
}

/// One call in flight; marks the session used when it ends.
struct InCall(Arc<Used>);

impl Drop for InCall {
    fn drop(&mut self) {
        *self.0.last.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
        self.0.calls.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The client side of a session: sends the relay's progress back to whoever asked for it.
#[derive(Clone)]
struct Forward(Progress);

impl ClientHandler for Forward {
    async fn on_progress(
        &self,
        mut params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let key = token_key(&params.progress_token);
        let target = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned();
        if let Some((token, peer)) = target {
            params.progress_token = token;
            let _ = peer.notify_progress(params).await;
        }
    }
}

impl Relays {
    pub(crate) fn new(relays: Vec<Relay>, idle: Duration, start_timeout: Duration) -> Self {
        Self {
            list: relays,
            idle,
            start_timeout,
            sessions: Mutex::default(),
            progress: Arc::default(),
            next_token: AtomicU64::new(0),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Every relay's tools for the caller, starting the sessions it lacks; a relay that cannot
    /// start is left out and logged.
    pub(crate) async fn tools(
        &self,
        caller: &Caller,
        facts: &Facts,
        taken: &[String],
    ) -> Vec<Tool> {
        let mut tools = Vec::new();
        for index in 0..self.list.len() {
            let slot = self.slot(caller.box_id.get(), index);
            let mut session = slot.lock().await;
            match self.ready(&mut session, caller, index, facts, taken).await {
                Ok(s) => tools.extend(s.tools.iter().cloned()),
                Err(e) => eprintln!(
                    "moor-machine-runner: relay {} for Box {}: {e}",
                    self.list[index].name,
                    caller.box_id.get()
                ),
            }
        }
        tools
    }

    /// Relays the call to the tool listed as `request.name`, with the forced arguments set
    /// from `facts`; the relay's result or error as it sent it.
    pub(crate) async fn call(
        &self,
        caller: &Caller,
        facts: &Facts,
        taken: &[String],
        mut request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let listed = request.name.to_string();
        let unknown = || ErrorData::invalid_params(format!("no tool named {listed}"), None);
        let index = self
            .list
            .iter()
            .position(|r| listed.starts_with(&format!("{}_", mcp_name(&r.name))))
            .ok_or_else(unknown)?;
        let relay = &self.list[index].name;
        let (peer, route, used) = {
            let slot = self.slot(caller.box_id.get(), index);
            let mut session = slot.lock().await;
            let session = self
                .ready(&mut session, caller, index, facts, taken)
                .await
                .map_err(|e| ErrorData::internal_error(format!("relay {relay}: {e}"), None))?;
            let route = session.routes.get(&listed).cloned().ok_or_else(unknown)?;
            (
                session.service.peer().clone(),
                route,
                Arc::clone(&session.used),
            )
        };
        let arguments = route.arguments(&listed, facts, request.arguments.take())?;
        request.name = route.name.into();
        request.arguments = Some(arguments);
        let key = self.track_progress(&mut request, &context.peer);
        used.calls.fetch_add(1, Ordering::SeqCst);
        let in_call = InCall(used);
        let result = tokio::select! {
            r = peer.call_tool_once(request) => r,
            () = context.ct.cancelled() => Err(ServiceError::Cancelled { reason: None }),
        };
        drop(in_call);
        if let Some(key) = key {
            self.progress
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&key);
        }
        result.map_err(|e| match e {
            ServiceError::McpError(e) => e,
            e => ErrorData::internal_error(format!("relay {relay}: {e}"), None),
        })
    }

    /// Ends every session that sat idle too long or whose server exited.
    pub(crate) async fn expire(&self) {
        let slots: Vec<Slot> = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for slot in slots {
            let Ok(mut session) = slot.try_lock() else {
                continue;
            };
            let stale = session.as_mut().is_some_and(|s| {
                let idle = s.used.calls.load(Ordering::SeqCst) == 0
                    && s.used
                        .last
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .elapsed()
                        > self.idle;
                idle || !s.alive()
            });
            if stale {
                if let Some(s) = session.take() {
                    s.end().await;
                }
            }
        }
    }

    /// Ends Box `id`'s sessions.
    pub(crate) async fn end_box(&self, id: i64) {
        let slots: Vec<Slot> = {
            let mut sessions = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
            let keys: Vec<_> = sessions.keys().filter(|(b, _)| *b == id).copied().collect();
            keys.iter().filter_map(|k| sessions.remove(k)).collect()
        };
        for slot in slots {
            if let Some(s) = slot.lock().await.take() {
                s.end().await;
            }
        }
    }

    /// Swaps the caller's progress token in `request` for one of the runner's own, which
    /// [`Forward`] maps back to the caller; that token's key, to drop once the call ends.
    fn track_progress(
        &self,
        request: &mut CallToolRequestParams,
        peer: &Peer<RoleServer>,
    ) -> Option<String> {
        let asked = request
            .meta
            .as_ref()
            .and_then(rmcp::model::RequestMetaObject::get_progress_token)?;
        let key = format!("relay-{}", self.next_token.fetch_add(1, Ordering::SeqCst));
        self.progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone(), (asked, peer.clone()));
        let ours = ProgressToken(NumberOrString::String(key.clone().into()));
        request
            .meta
            .get_or_insert_with(Default::default)
            .set_progress_token(ours);
        Some(key)
    }

    fn slot(&self, id: i64, index: usize) -> Slot {
        let mut sessions = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(sessions.entry((id, index)).or_default())
    }

    /// The live session in `slot`, started first if there is none or its server exited.
    async fn ready<'s>(
        &self,
        slot: &'s mut Option<Session>,
        caller: &Caller,
        index: usize,
        facts: &Facts,
        taken: &[String],
    ) -> Result<&'s Session, String> {
        if let Some(dead) = slot.take_if(|s| !s.alive()) {
            dead.end().await;
        }
        if slot.is_none() {
            let start = self.start(caller, &self.list[index], facts, taken);
            let session = tokio::time::timeout(self.start_timeout, start)
                .await
                .map_err(|_| "timed out starting".to_owned())??;
            *slot = Some(session);
        }
        slot.as_ref().ok_or_else(|| "no session".to_owned())
    }

    /// Starts the relay on the caller's machine and lists its tools.
    async fn start(
        &self,
        caller: &Caller,
        relay: &Relay,
        facts: &Facts,
        taken: &[String],
    ) -> Result<Session, String> {
        let dir = caller.remote_dir();
        let body = format!(
            "mkdir -p ~/{dir} && cd ~/{dir} && exec \"$SHELL\" -lc {}",
            exec::quote(&relay.command)
        );
        let env = hooks::box_env(caller.box_id.get(), caller.slot, facts);
        let mut child = Command::new("ssh")
            .args(["-o", "BatchMode=yes", "--", &caller.ssh])
            .arg(exec::script(&env, &body))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("could not run ssh: {e}"))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err("ssh has no stdio".to_owned());
        };
        let service = Forward(Arc::clone(&self.progress))
            .serve((stdout, stdin))
            .await
            .map_err(|e| format!("could not start: {e}"))?;
        let listed = service
            .peer()
            .list_all_tools()
            .await
            .map_err(|e| format!("could not list its tools: {e}"))?;
        let (tools, routes) = expose(relay, listed, taken)?;
        Ok(Session {
            service,
            child,
            tools,
            routes,
            used: Arc::new(Used {
                last: Mutex::new(Instant::now()),
                calls: AtomicUsize::new(0),
            }),
        })
    }
}

impl Session {
    /// Whether its server still runs.
    fn alive(&mut self) -> bool {
        !self.service.is_closed() && matches!(self.child.try_wait(), Ok(None))
    }

    /// Closes the server's stdin and waits up to 5 s for ssh to exit, then kills it.
    async fn end(mut self) {
        let _ = self.service.cancel().await;
        let wait = tokio::time::timeout(Duration::from_secs(5), self.child.wait());
        if wait.await.is_err() {
            let _ = self.child.kill().await;
        }
    }
}

/// A progress token as a map key.
fn token_key(token: &ProgressToken) -> String {
    match &token.0 {
        NumberOrString::Number(n) => n.to_string(),
        NumberOrString::String(s) => s.to_string(),
    }
}
