//! Front conversation bindings: `bind-session` and `session-binding`.

use moor_core::request::{BindSessionReq, SessionBindingReq};
use moor_core::view::binding::{BindingValue, BoundValue};
use moor_core::{ErrorKind, SessionId};

use super::host_session::refuse_invalid;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::Binding;

#[cfg(test)]
mod tests;

/// Run `bind-session`.
///
/// Reads the binding back after the write, so a lost race returns the existing binding
/// with `created` false.
///
/// # Errors
/// [`CommandError`] when the front or key is empty, the Session is unknown or ended, or the
/// store fails.
pub(crate) fn bind(cx: &Cx, req: &BindSessionReq) -> Result<BoundValue, CommandError> {
    check(&req.front, &req.key)?;
    let store = cx.store()?;
    let created = store
        .bind_session(&req.front, &req.key, req.session.get(), &req.owner)
        .map_err(refuse_invalid)?;
    let binding = store.binding(&req.front, &req.key)?.ok_or_else(|| {
        CommandError::new(
            ErrorKind::Internal,
            "binding missing right after it was bound",
        )
    })?;
    Ok(BoundValue {
        created,
        binding: value(binding),
    })
}

/// Run `session-binding`.
///
/// # Errors
/// [`CommandError`] when the front or key is empty, or the store read fails.
pub(crate) fn lookup(
    cx: &Cx,
    req: &SessionBindingReq,
) -> Result<Option<BindingValue>, CommandError> {
    check(&req.front, &req.key)?;
    Ok(cx.store()?.binding(&req.front, &req.key)?.map(value))
}

/// The wire view of a store binding.
fn value(b: Binding) -> BindingValue {
    BindingValue {
        front: b.front,
        key: b.key,
        session: SessionId::new(b.session),
        task: b.task.map(|t| t.as_str().to_owned()),
        owner: b.owner,
    }
}

/// A binding needs both a front and a key.
fn check(front: &str, key: &str) -> Result<(), CommandError> {
    if front.is_empty() || key.is_empty() {
        return Err(CommandError::new(
            ErrorKind::Usage,
            "pass a non-empty front and key",
        ));
    }
    Ok(())
}
