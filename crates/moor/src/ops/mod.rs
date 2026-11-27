//! Command operations: one op per request.

use moor_core::view::user::MachineKeysValue;
use moor_core::{ErrorKind, Request};

use crate::cx::Cx;
use crate::error::CommandError;

pub(crate) mod artifacts;
mod ask;
pub(crate) mod attach;
pub(crate) mod authz;
mod auto_close;
mod binding;
pub(crate) mod box_config;
pub(crate) mod box_rsync;
pub(crate) mod box_tool;
mod changes;
mod close;
pub(crate) mod close_failed;
pub(crate) mod common;
mod cp;
mod end_session;
pub(crate) mod env;
pub(crate) mod exec;
pub(crate) mod forward;
mod gate;
mod github_app;
pub(crate) mod github_hook;
pub(crate) mod host_session;
mod inspect;
mod machine_box;
pub(crate) mod machine_hold;
pub(crate) mod machine_pair;
mod node_add;
mod node_settings;
mod nodes;
mod open_session;
pub(crate) mod output_report;
mod place;
pub(crate) mod pulls;
mod repo_config;
mod retry;
mod run;
mod sessions;
pub(crate) mod show;
mod show_session;
mod sleep;
mod start;
mod steer;
pub(crate) mod users;
mod version;
mod wait;
mod wait_events;
mod wake;
pub(crate) mod webhook_outbox;
pub(crate) mod workflow;

pub(crate) use auto_close::{pass as auto_close_pass, Looked as AutoCloseLooked};
pub(crate) use close::wanted::pass as wanted_pass;
pub(crate) use start::front::ASK_INSTEAD;
pub(crate) use common::promote::pass as promote_pass;
pub(crate) use gate::checks::{pass as gate_pass, run as run_gate_check, Job as GateJob};
#[cfg(test)]
pub(crate) use run::tests as run_fixture;
use run::Sink;
pub(crate) use start::settle_all;
pub(crate) use steer::{pass as inbox_pass, Backoff as InboxBackoff};

#[cfg(test)]
mod tests;

/// Why `artifact-read` is refused here.
const RAW_BYTES: &str = "artifact-read sends raw bytes, so only the controller socket serves it";

/// Route a request to its op.
#[expect(clippy::too_many_lines, reason = "one arm per request")]
pub(crate) fn dispatch(cx: &Cx, req: Request) -> Result<serde_json::Value, CommandError> {
    match req {
        Request::Version {} => version_value(),
        Request::Start(r) => to_value(&start::front::from_front(cx, r)?),
        Request::Ask(r) => to_value(&ask::run(cx, &r)?),
        Request::StartNeeds(r) => to_value(&start::needs(cx, &r)?),
        Request::Show(r) => to_value(&show::run(cx, &r)?),
        Request::Inspect(r) => to_value(&inspect::run(cx, &r)?),
        Request::Changes(r) => to_value(&changes::run(cx, &r)?),
        Request::ShowSession(r) => to_value(&show_session::run(cx, &r)?),
        Request::Artifacts(r) => to_value(&artifacts::list(cx, &r)?),
        req @ (Request::ArtifactRead(_)
        | Request::Attach(_)
        | Request::BoxRsync(_)
        | Request::Exec(_)
        | Request::Forward(_)
        | Request::As(_)
        | Request::Whoami(_)
        | Request::Enroll(_)
        | Request::SecretSet(_)
        | Request::SecretDelete(_)
        | Request::SecretNames(_)
        | Request::McpAdd(_)
        | Request::McpRemove(_)
        | Request::McpList(_)
        | Request::ProfileSet(_)
        | Request::ProfileShow(_)
        | Request::ProfileCheck(_)
        | Request::ProfileRemove(_)
        | Request::WebhookAdd(_)
        | Request::WebhookSetupShow(_)
        | Request::WebhookSetupFinish(_)
        | Request::WebhookList(_)
        | Request::WebhookRemove(_)
        | Request::Inbox(_)
        | Request::WorkflowAdd(_)
        | Request::WorkflowList(_)
        | Request::WorkflowRemove(_)
        | Request::WorkflowSetEnabled(_)
        | Request::WorkflowRuns(_)
        | Request::WorkflowRunList(_)
        | Request::WorkflowRunRead(_)
        | Request::WorkflowAnswer(_)
        | Request::WorkflowRunNow(_)
        | Request::McpOAuthFinish(_)
        | Request::MachineRemove(_)
        | Request::MachineEnroll(_)
        | Request::MachineList(_)
        | Request::MachineStatus(_)) => Err(socket_only(&req)),
        Request::WebhookClaim(_) | Request::WebhookReport(_) => Err(CommandError::new(
            ErrorKind::Refused,
            webhook_outbox::FRONT_ONLY,
        )),
        Request::StepReport(_) | Request::OutputReport(_) | Request::ReadingHandIn(_) => Err(
            CommandError::new(ErrorKind::Refused, authz::STEP_REPORT_FRONT_ONLY),
        ),
        Request::BoxTool(_) => Err(CommandError::new(ErrorKind::Refused, box_tool::FRONT_ONLY)),
        Request::MachinePair(_) => Err(CommandError::new(
            ErrorKind::Refused,
            machine_pair::FRONT_ONLY,
        )),
        Request::GitHubHook(_) => Err(CommandError::new(
            ErrorKind::Refused,
            github_hook::FRONT_ONLY,
        )),
        Request::MachineKeys {} => to_value(&MachineKeysValue {
            machines: cx.store()?.machine_keys()?,
            shared: cx.store()?.shared_machine_keys()?,
            controller: Some(cx.store()?.controller_id()?),
        }),
        Request::MachineLinks(r) => {
            for stale in cx.store()?.set_box_memory(&r.runner, &r.box_memory)? {
                common::stale::unless_stale(cx, Err::<(), _>(stale))?;
            }
            to_value(&nodes::set_links(cx, &r)?)
        }
        Request::MachineBox(r) => to_value(&machine_box::run(cx, r.op)?),
        Request::NodeList(_) => to_value(&nodes::list(cx)?),
        Request::GitHubApp(_) => to_value(&github_app::installed(cx)?),
        Request::NodeAdd(r) => to_value(&node_add::run(cx, &r)?),
        Request::NodeSettingsGet(r) => to_value(&node_settings::get(cx, &r)?),
        Request::NodeSettingsSet(r) => to_value(&node_settings::set(cx, &r)?),
        Request::Sessions(r) => to_value(&sessions::run(cx, &r)?),
        Request::Close(r) => to_value(&close::requested(cx, &r)?),
        Request::ForwardOpen(r) => to_value(&forward::open(cx, &r)?),
        Request::ForwardClose(r) => to_value(&forward::close(cx, &r)?),
        Request::Gate(r) => to_value(&gate::run(cx, &r)?),
        Request::Env(r) => to_value(&env::run(cx, &r)?),
        Request::RepoConfigGet(r) => to_value(&repo_config::get(cx, &r)?),
        Request::RepoConfigSet(r) => to_value(&repo_config::set(cx, &r)?),
        Request::BoxConfigSet(r) => to_value(&box_config::set(cx, &r)?),
        Request::Run(r) => to_value(&run::run(cx, &r)?),
        Request::Wait(r) => to_value(&wait::run(cx, &r)?),
        Request::WaitEvents(r) => to_value(&wait_events::run(cx, &r)?),
        Request::Cp(r) => to_value(&cp::run(cx, &r)?),
        Request::MachineHold(r) => to_value(&machine_hold::run(cx, &r)?),
        Request::Sleep(r) => to_value(&sleep::run(cx, &r)?),
        Request::Wake(r) => to_value(&wake::run(cx, &r)?),
        Request::Retry(r) => to_value(&retry::run(cx, &r)?),
        Request::Pulls(r) => to_value(&pulls::run(cx, &r)?),
        Request::CloseFailed(r) => to_value(&close_failed::run(cx, &r)?),
        Request::Steer(r) => to_value(&steer::run(cx, &r)?),
        Request::GateAnswer(r) => to_value(&gate::answer::run(cx, &r)?),
        Request::EndSession(r) => to_value(&end_session::run(cx, &r)?),
        Request::OpenSession(r) => to_value(&open_session::run(cx, &r)?),
        Request::OpenHostSession(r) => to_value(&host_session::open(cx, &r)?),
        Request::UpdateHostSession(r) => to_value(&host_session::update(cx, &r)?),
        Request::EndHostSession(r) => to_value(&host_session::end(cx, &r)?),
        Request::BindSession(r) => to_value(&binding::bind(cx, &r)?),
        Request::SessionBinding(r) => to_value(&binding::lookup(cx, &r)?),
    }
}

/// `step-report`: the agent's report for its Box's run, answered with what to do next.
pub(crate) fn step_report(
    cx: &Cx,
    req: &moor_core::request::StepReportReq,
) -> Result<serde_json::Value, CommandError> {
    to_value(&workflow::step_report(cx, req)?)
}

/// The refusal for `req`, a request only the controller socket serves.
fn socket_only(req: &Request) -> CommandError {
    let (kind, why) = match req {
        Request::ArtifactRead(_) => (ErrorKind::Usage, RAW_BYTES),
        Request::Attach(_) => (
            ErrorKind::Refused,
            "attach relays a terminal both ways, so only the controller socket serves it",
        ),
        Request::Exec(_) => (
            ErrorKind::Refused,
            "exec streams a command's stdio both ways, so only the controller socket serves it",
        ),
        Request::BoxRsync(_) => (
            ErrorKind::Refused,
            "box-rsync streams rsync both ways, so only the controller socket serves it",
        ),
        Request::Forward(_) => (
            ErrorKind::Refused,
            "forward carries a connection both ways, so only the controller socket serves it",
        ),
        Request::As(_) => (
            ErrorKind::Usage,
            "as acts as an identity a socket checks, so only the controller socket serves it",
        ),
        _ => (
            ErrorKind::Usage,
            "user requests act as the caller of a socket, so only the controller socket serves them",
        ),
    };
    CommandError::new(kind, why)
}

/// `artifact-read`: stream the Artifact's bytes to `out`, then the response value.
pub(crate) fn read_artifact(
    cx: &Cx,
    req: &moor_core::ArtifactReadReq,
    out: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
) -> Result<serde_json::Value, CommandError> {
    to_value(&artifacts::read(cx, req, out)?)
}

/// `run`: pass the command's output to `out` as it runs, then the response value.
pub(crate) fn run_streamed(
    cx: &Cx,
    req: &moor_core::RunReq,
    out: Sink<'_>,
) -> Result<serde_json::Value, CommandError> {
    to_value(&run::streamed(cx, req, Some(out))?)
}

/// The `version` result; needs no context, so it works without config.
pub(crate) fn version_value() -> Result<serde_json::Value, CommandError> {
    serde_json::to_value(version::exec()).map_err(|e| CommandError::new(ErrorKind::Internal, e))
}

/// Serialise a view into the response value.
fn to_value<T: serde::Serialize>(view: &T) -> Result<serde_json::Value, CommandError> {
    serde_json::to_value(view).map_err(|e| CommandError::new(ErrorKind::Internal, e))
}
