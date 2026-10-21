//! Who may act on a Box: the user it acts for, and the local operator. Every request the
//! controller serves passes [`authorize`] before it runs.

use moor_core::request::{Grant, Named, Pins};
use moor_core::user::{Caller, Identity, Viewer};
use moor_core::{BoxId, BoxName, BoxTarget, ErrorKind, Request};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// The one refusal for a Box the caller may not act on and for a Box that does not exist, so
/// that names and ids cannot be probed.
pub(crate) const NOT_YOURS: &str = "not your Box";

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
/// [`NOT_YOURS`] for a Box the caller may not act on or that does not exist, the refusal for
/// an identity the caller may not assert, or a store failure.
pub(crate) fn authorize(
    cx: &Cx,
    caller: &Result<Caller, String>,
    asserted: Option<&Identity>,
    request: &Request,
) -> Result<Grant, CommandError> {
    let names = request.names();
    let viewer = match viewer(cx, caller, asserted) {
        Ok(viewer) => viewer,
        Err(_) if names.is_empty() && asserted.is_none() => Viewer::Nobody,
        Err(e) => return Err(e),
    };
    let mut pins = Pins::new();
    if viewer != Viewer::Operator {
        let store = cx.store()?;
        for named in &names {
            if !may_name(store, viewer, named, &mut pins)? {
                return Err(CommandError::new(ErrorKind::Refused, NOT_YOURS));
            }
        }
    }
    Ok(Grant { viewer, pins })
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
        Named::Session(id) => match store.session_row(id.get())?.map(|s| s.box_id) {
            Some(Some(id)) => store.box_row(id)?.map(|row| row.user),
            Some(None) => return Ok(true),
            None => None,
        },
    };
    Ok(owner.is_some_and(|owner| viewer.may_act_on(owner)))
}
