//! `machine_claim` and `machine_release`: the Box asks the controller for a slot on one of its
//! user's machines, or gives its slot back. The Proxy reads the hold on every request, so the
//! call after a claim already carries the machine headers.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use moor_client::Client;
use moor_core::request::{HoldOp, HoldValue, MachineHoldReq, Request};
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, ErrorData, RoleServer};
use serde::{Deserialize, Serialize};

use crate::beat::Progress;
use crate::caller::Caller;
use crate::hooks::Facts;
use crate::Runner;

/// Longest `wait_secs` a claim may ask for.
const MAX_WAIT_SECS: u32 = 3600;
/// How often a waiting claim asks again.
const POLL: Duration = Duration::from_secs(2);

/// Arguments of `machine_claim`.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub(crate) struct ClaimArgs {
    /// Labels the machine must all have; the repo's `machine:` labels when left out.
    #[serde(default)]
    labels: Option<Vec<String>>,
    /// The one machine wanted, by name.
    #[serde(default)]
    machine: Option<String>,
    /// Box ports to forward to the machine; the repo's when left out.
    #[serde(default)]
    ports: Option<Vec<u16>>,
    /// How long to wait for a free slot, in seconds: 0 (the default) refuses at once, at most 3600.
    #[serde(default)]
    wait_secs: Option<u32>,
}

/// The slot a claim holds.
#[derive(Debug, Serialize)]
struct Claimed {
    /// The machine's name.
    machine: String,
    /// The Box's slot on it.
    slot: u16,
    /// Add this to a repo's port for the Box's port on the machine: 10 times the slot.
    port_offset: u32,
    /// The forwarded ports, moved by the slot.
    ports: Vec<u16>,
    /// What the machine's operator set up for this Box, as `machine_info` gives them.
    facts: Facts,
}

#[tool_router(router = claim_router, vis = "pub(crate)")]
impl Runner {
    /// Claims a slot on one of the user's machines for the calling Box.
    #[tool(
        description = "Claim a slot on one of your user's machines, so sync, exec, pull and the \
                       machine's relayed tools work. labels and ports default to the repo's \
                       machine: section; machine names one machine. With no free slot it \
                       fails at once, naming the Boxes on the machine, unless wait_secs (at \
                       most 3600) asks it to wait, reporting its place in line. Returns \
                       machine, slot, port_offset, ports and facts, as machine_info does. A \
                       claim while holding a slot returns that slot."
    )]
    async fn machine_claim(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<ClaimArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let id = Caller::box_of(&crate::parts(&context)?.headers)?;
        let wait = args.wait_secs.unwrap_or(0);
        if wait > MAX_WAIT_SECS {
            return Ok(failed(format!(
                "wait_secs must be at most {MAX_WAIT_SECS}, not {wait}"
            )));
        }
        let progress = Progress::of(&context);
        let held = tokio::select! {
            held = claim(&self.client, id, args, &progress) => held,
            () = context.ct.cancelled() => {
                let _ = ask(&self.client, id, HoldOp::Release).await;
                return Ok(failed("the caller went away".to_owned()));
            }
        };
        let claimed = match held {
            Ok(caller) => self.claimed(caller).await,
            Err(reason) => return Ok(failed(reason)),
        };
        let claimed = match claimed {
            Ok(claimed) => claimed,
            Err(reason) => {
                let reason = match ask(&self.client, id, HoldOp::Release).await {
                    Ok(_) => format!("{reason}; the slot is released"),
                    Err(e) => format!("{reason}; releasing the slot failed too: {e}"),
                };
                return Ok(failed(reason));
            }
        };
        let _ = context.peer.notify_tool_list_changed().await;
        Ok(claimed)
    }

    /// Gives back the calling Box's slot, suspending the Box on the machine first.
    #[tool(
        description = "Give back this Box's slot on its machine, so another Box can take it. \
                       The machine's suspend step runs first; your files there stay until the \
                       Box closes, and a later machine_claim finds them. Returns the machine \
                       released, or null when the Box held none."
    )]
    async fn machine_release(
        &self,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let id = Caller::box_of(&parts.headers)?;
        if let Err(e) = self
            .hooks
            .suspend(id.get(), &self.relays, &self.holds)
            .await
        {
            return Ok(failed(format!("suspend failed, so the slot is kept: {e}")));
        }
        Ok(match ask(&self.client, id, HoldOp::Release).await {
            Ok(HoldValue::Released { machine }) => {
                CallToolResult::structured(serde_json::json!({ "released": machine }))
            }
            Ok(other) => failed(format!("machine-hold answered a release with {other:?}")),
            Err(e) => failed(e),
        })
    }
}

impl Runner {
    /// The answer to a claim that holds `caller`'s slot, with the Box's facts there, or why the
    /// machine can't be used.
    async fn claimed(&self, caller: (Caller, Vec<u16>)) -> Result<CallToolResult, String> {
        let (caller, ports) = caller;
        match self.hooks.facts(&caller).await {
            Ok(facts) => {
                // Starting a relay takes seconds; started here, the client's re-list after the
                // claim finds its tools at once instead of after a later call.
                self.relays
                    .tools(&caller, &facts, &crate::own_names())
                    .await;
                Ok(CallToolResult::structured(serde_json::json!(Claimed {
                    port_offset: 10 * u32::from(caller.slot),
                    machine: caller.machine,
                    slot: caller.slot,
                    ports,
                    facts,
                })))
            }
            Err(e) => Err(format!("{} can't be used: {e}", caller.machine)),
        }
    }
}

/// Asks for a slot until the controller holds one, refuses, or the wait runs out; a wait that
/// runs out lets go of its place in line.
async fn claim(
    client: &Client,
    id: BoxId,
    args: ClaimArgs,
    progress: &Progress,
) -> Result<(Caller, Vec<u16>), String> {
    let wait = Duration::from_secs(u64::from(args.wait_secs.unwrap_or(0)));
    let deadline = tokio::time::Instant::now() + wait;
    let wanted = args
        .machine
        .clone()
        .unwrap_or_else(|| "a machine".to_owned());
    let op = HoldOp::Claim {
        labels: args.labels.unwrap_or_default(),
        machine: args.machine,
        ports: args.ports,
        wait_until: (!wait.is_zero()).then(|| now().saturating_add(wait)),
    };
    loop {
        let reason = match ask(client, id, op.clone()).await? {
            HoldValue::Held {
                machine,
                ssh,
                slot,
                ports,
            } => {
                let sync_exclude = Vec::new();
                let caller = Caller {
                    box_id: id,
                    machine,
                    ssh,
                    slot,
                    sync_exclude,
                };
                return Ok((caller, ports));
            }
            HoldValue::Refused { reason } => return Err(format!("no free slot: {reason}")),
            HoldValue::Released { .. } => return Err("the claim was let go of".to_owned()),
            HoldValue::Waiting { position, reason } => {
                progress
                    .note(format!("waiting for {wanted}: place {position} in line"))
                    .await;
                reason
            }
        };
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            let _ = ask(client, id, HoldOp::Release).await;
            return Err(format!("no free slot: {reason}"));
        }
        tokio::time::sleep(left.min(POLL)).await;
    }
}

/// Sends `op` for Box `id` as a `machine-hold` and reads the answer.
async fn ask(client: &Client, id: BoxId, op: HoldOp) -> Result<HoldValue, String> {
    let client = client.clone();
    let request = Request::MachineHold(MachineHoldReq { id, op });
    let response = tokio::task::spawn_blocking(move || client.request(request, |_| {}))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("the controller did not answer machine-hold: {e}"))?;
    if let Some(error) = response.error {
        return Err(format!(
            "the controller refused machine-hold: {}",
            error.message
        ));
    }
    serde_json::from_value(response.value.unwrap_or_default())
        .map_err(|e| format!("machine-hold answered something else: {e}"))
}

/// A tool result that fails with `reason`.
fn failed(reason: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(reason)])
}

#[allow(
    clippy::disallowed_methods,
    reason = "the controller compares wait_until with its wall clock; the runner has no injected clock"
)]
pub(crate) fn now() -> Ts {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    Ts::from_millis(ms)
}

/// Clients re-list tools on `tools/list_changed` only from a server that says its list can
/// change, and a claim adds the machine's relayed tools.
pub(crate) fn server_config() -> ServerConfig {
    ServerConfig::new(
        ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .build(),
    )
}
