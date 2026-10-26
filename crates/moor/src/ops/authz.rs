//! Who may act on a Box: the user it acts for, and the local operator. Every request the
//! controller serves passes [`authorize`] before it runs.

use moor_core::request::{Grant, Named, Pins};
use moor_core::user::{Caller, Identity, Viewer};
use moor_core::{BoxId, BoxName, BoxTarget, ErrorKind, Request};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, SessionRecord, Store};

/// The one refusal for a Box the caller may not act on and for a Box that does not exist, so
/// that names and ids cannot be probed.
pub(crate) const NOT_YOURS: &str = "not your Box";

/// The refusal for a `cp` from anyone but the operator: it writes on the controller host.
pub(crate) const CP_OPERATOR_ONLY: &str = "cp to the host is operator-only";

/// The refusal for a `box-mirror` from anyone but the operator: it writes on the host.
pub(crate) const MIRROR_OPERATOR_ONLY: &str = "box-mirror to the host is operator-only";

/// The refusal for a `box-put` from anyone but the operator: it reads a host file.
pub(crate) const PUT_OPERATOR_ONLY: &str = "box-put from the host is operator-only";

/// The refusal for an `env` from anyone but the operator: it reads host configs.
pub(crate) const ENV_OPERATOR_ONLY: &str = "env reads the host's configs, which is operator-only";

/// The refusal for a `machine-targets` from anyone but the operator: it spans every user.
pub(crate) const TARGETS_OPERATOR_ONLY: &str =
    "machine-targets lists every user's machines, which is operator-only";

/// Whose Boxes `caller` may see, asserting `asserted`. Only the controller socket's own uid
/// is the operator; a front socket is a user at most, whatever it asserts.
///
/// # Errors
/// The refusal for an identity `caller` may not assert, or a store failure.
pub(crate) fn viewer(
    cx: &Cx,
    caller: &Result<Caller, String>,
    asserted: Option<&Identity>,
) -> Result<Viewer, CommandError> {
    let refused = |why: &String| CommandError::new(ErrorKind::Refused, why.clone());
    let caller = caller.as_ref().map_err(refused)?;
    let who = caller.resolve(asserted).map_err(|why| refused(&why))?;
    if matches!(caller, Caller::Operator(_)) {
        return Ok(Viewer::Operator);
    }
    Ok(cx
        .store()?
        .user_of(&who)?
        .map_or(Viewer::Nobody, Viewer::User))
}

/// Refuses `request` unless its caller may act on every Box it names, and returns what the
/// request may see.
///
/// # Errors
/// [`NOT_YOURS`] for a Box the caller may not act on or that does not exist, the refusal for a
/// request that touches the host and an identity the caller may not assert, or a store failure.
pub(crate) fn authorize(
    cx: &Cx,
    caller: &Result<Caller, String>,
    asserted: Option<&Identity>,
    request: &Request,
) -> Result<Grant, CommandError> {
    let viewer = viewer(cx, caller, asserted)?;
    let mut pins = Pins::new();
    if viewer != Viewer::Operator {
        let store = cx.store()?;
        for named in &request.names() {
            if !may_name(store, viewer, named, &mut pins)? {
                return Err(CommandError::new(ErrorKind::Refused, NOT_YOURS));
            }
        }
        if let Some(why) = host_only(request) {
            return Err(CommandError::new(ErrorKind::Refused, why));
        }
    }
    Ok(Grant { viewer, pins })
}

/// Why only the operator may send `request`, when it touches the controller host or every
/// user's data. Every kind is listed, so a new kind must declare its reach here before it
/// compiles.
const fn host_only(request: &Request) -> Option<&'static str> {
    match request {
        Request::Cp(_) => Some(CP_OPERATOR_ONLY),
        Request::BoxMirror(_) => Some(MIRROR_OPERATOR_ONLY),
        Request::BoxPut(_) => Some(PUT_OPERATOR_ONLY),
        Request::Env(_) => Some(ENV_OPERATOR_ONLY),
        Request::MachineTargets {} => Some(TARGETS_OPERATOR_ONLY),
        Request::Start(_)
        | Request::StartNeeds(_)
        | Request::Run(_)
        | Request::Wait(_)
        | Request::WaitEvents(_)
        | Request::Steer(_)
        | Request::EndSession(_)
        | Request::OpenSession(_)
        | Request::OpenHostSession(_)
        | Request::UpdateHostSession(_)
        | Request::EndHostSession(_)
        | Request::BindSession(_)
        | Request::SessionBinding(_)
        | Request::ShowSession(_)
        | Request::Sessions(_)
        | Request::Show(_)
        | Request::Close(_)
        | Request::Open(_)
        | Request::Sleep(_)
        | Request::Wake(_)
        | Request::Connect(_)
        | Request::Gate(_)
        | Request::Artifacts(_)
        | Request::ArtifactRead(_)
        | Request::Whoami(_)
        | Request::Enroll(_)
        | Request::SecretSet(_)
        | Request::SecretDelete(_)
        | Request::SecretNames(_)
        | Request::McpAdd(_)
        | Request::McpRemove(_)
        | Request::McpList(_)
        | Request::McpOAuthFinish(_)
        | Request::MachineAdd(_)
        | Request::MachineRemove(_)
        | Request::MachineList(_)
        | Request::LinkCode(_)
        | Request::LinkRedeem(_)
        | Request::ProxyLease(_)
        | Request::ProxyRelease(_)
        | Request::ProxyStatus(_)
        | Request::As(_)
        | Request::Version {} => None,
    }
}

/// Whether the context's viewer may see Box `id`; a Box that does not exist counts as no.
///
/// # Errors
/// A store failure.
pub(crate) fn may_see(cx: &Cx, store: &Store, id: BoxId) -> Result<bool, CommandError> {
    let viewer = cx.viewer();
    if viewer == Viewer::Operator {
        return Ok(true);
    }
    Ok(store
        .box_row(id)?
        .is_some_and(|row| viewer.may_act_on(row.user)))
}

/// Whether the context's viewer may see `session`: a Box's Session with its Box, a host
/// Session when it belongs to the viewer's user.
///
/// # Errors
/// A store failure.
pub(crate) fn may_see_session(
    cx: &Cx,
    store: &Store,
    session: &SessionRecord,
) -> Result<bool, CommandError> {
    session
        .box_id
        .map_or(Ok(cx.viewer().may_act_on(session.user)), |id| {
            may_see(cx, store, id)
        })
}

/// Whether `viewer` may act on what `named` names, pinning each name to the Box it means; a
/// Box or Session that does not exist counts as no.
fn may_name(
    store: &Store,
    viewer: Viewer,
    named: &Named,
    pins: &mut Pins,
) -> Result<bool, CommandError> {
    let mut pin = |name: &BoxName, row: BoxRow| {
        pins.insert(name.clone(), row.id);
        row.user
    };
    let owner = match named {
        Named::Box(BoxTarget::Id(id)) => store.box_row(*id)?.map(|row| row.user),
        // A name has one open Box at most and it is the newest of that name, so the newest is
        // the Box every handler's rule picks when it picks one.
        Named::Box(BoxTarget::Name(name)) => store.latest_box_by_name(name)?.map(|r| pin(name, r)),
        Named::Attach(name) => match store.open_box_by_name(name)? {
            Some(row) => Some(pin(name, row)),
            None => return Ok(true),
        },
        // A host Session has no Box: it belongs to the user who opened it.
        Named::Session(id) => match store.session_row(id.get())? {
            Some(SessionRecord {
                box_id: Some(id), ..
            }) => store.box_row(id)?.map(|row| row.user),
            Some(host) => Some(host.user),
            None => None,
        },
    };
    Ok(owner.is_some_and(|owner| viewer.may_act_on(owner)))
}
