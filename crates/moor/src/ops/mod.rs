//! Command operations: one op per request.

use moor_core::view::user::{MachineKeysValue, MachineLinksValue, MachineTargetsValue};
use moor_core::{ErrorKind, Request};

use crate::cx::Cx;
use crate::error::CommandError;

mod artifacts;
pub(crate) mod attach;
pub(crate) mod authz;
mod binding;
mod box_mirror;
mod box_put;
mod close;
pub(crate) mod common;
mod cp;
mod end_session;
pub(crate) mod env;
mod front_proxy;
mod gate;
pub(crate) mod host_session;
pub(crate) mod machine_hold;
mod open;
mod open_session;
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

#[cfg(test)]
pub(crate) use run::tests as run_fixture;
use run::Sink;
pub(crate) use start::settle_all;

#[cfg(test)]
mod tests;

/// Why `artifact-read` is refused here.
const RAW_BYTES: &str = "artifact-read sends raw bytes, so only the controller socket serves it";

/// Route a request to its op.
#[expect(clippy::too_many_lines, reason = "one arm per request kind")]
pub(crate) fn dispatch(cx: &Cx, req: Request) -> Result<serde_json::Value, CommandError> {
    match req {
        Request::Version {} => version_value(),
        Request::Start(r) => to_value(&start::run(cx, &r)?),
        Request::StartNeeds(r) => to_value(&start::needs(cx, &r)?),
        Request::Show(r) => to_value(&show::run(cx, &r)?),
        Request::ShowSession(r) => to_value(&show_session::run(cx, &r)?),
        Request::Artifacts(r) => to_value(&artifacts::list(cx, &r)?),
        req @ (Request::ArtifactRead(_)
        | Request::Attach(_)
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
        | Request::ProfileRemove(_)
        | Request::McpOAuthFinish(_)
        | Request::MachineAdd(_)
        | Request::MachineRemove(_)
        | Request::MachineEnroll(_)
        | Request::MachineList(_)
        | Request::MachineStatus(_)
        | Request::ProxyLease(_)
        | Request::ProxyRelease(_)
        | Request::ProxyStatus(_)) => Err(socket_only(&req)),
        Request::MachineTargets {} => to_value(&MachineTargetsValue {
            targets: cx.store()?.machine_targets()?,
        }),
        Request::MachineKeys {} => to_value(&MachineKeysValue {
            machines: cx.store()?.machine_keys()?,
        }),
        Request::MachineLinks(r) => to_value(&MachineLinksValue {
            links: cx.store()?.set_machine_links(&r.links)?,
        }),
        Request::Sessions(r) => to_value(&sessions::run(cx, &r)?),
        Request::Close(r) => to_value(&close::run(cx, &r)?),
        Request::Gate(r) => to_value(&gate::run(cx, &r)?),
        Request::Env(r) => to_value(&env::run(cx, &r)?),
        Request::Run(r) => to_value(&run::run(cx, &r)?),
        Request::Wait(r) => to_value(&wait::run(cx, &r)?),
        Request::WaitEvents(r) => to_value(&wait_events::run(cx, &r)?),
        Request::Cp(r) => to_value(&cp::run(cx, &r)?),
        Request::BoxMirror(r) => to_value(&box_mirror::run(cx, &r)?),
        Request::BoxPut(r) => to_value(&box_put::run(cx, &r)?),
        Request::MachineHold(r) => to_value(&machine_hold::run(cx, &r)?),
        Request::Sleep(r) => to_value(&sleep::run(cx, &r)?),
        Request::Wake(r) => to_value(&wake::run(cx, &r)?),
        Request::Open(r) => to_value(&open::run(cx, &r)?),
        Request::Connect(r) => to_value(&open::connect(cx, &r)?),
        Request::Steer(r) => to_value(&steer::run(cx, &r)?),
        Request::EndSession(r) => to_value(&end_session::run(cx, &r)?),
        Request::OpenSession(r) => to_value(&open_session::run(cx, &r)?),
        Request::OpenHostSession(r) => to_value(&host_session::open(cx, &r)?),
        Request::UpdateHostSession(r) => to_value(&host_session::update(cx, &r)?),
        Request::EndHostSession(r) => to_value(&host_session::end(cx, &r)?),
        Request::BindSession(r) => to_value(&binding::bind(cx, &r)?),
        Request::SessionBinding(r) => to_value(&binding::lookup(cx, &r)?),
    }
}

/// The refusal for `req`, a request only the controller socket serves.
fn socket_only(req: &Request) -> CommandError {
    let (kind, why) = match req {
        Request::ArtifactRead(_) => (ErrorKind::Usage, RAW_BYTES),
        Request::Attach(_) => (
            ErrorKind::Refused,
            "attach relays a terminal both ways, so only the controller socket serves it",
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
