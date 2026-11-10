//! The turn a close takes on its Box, stopping a start in flight first when the close is forced.

use moor_core::domain::decision::{Decision, Effect, WantChange, Wanted, WantedKind};
use moor_core::ErrorKind;

use super::cancel_boot;
use crate::cx::owners::{Cmd, Turn, INTERRUPT_WAIT};
use crate::cx::{Cx, QUEUE_WAIT};
use crate::error::CommandError;
use crate::store::BoxRow;

/// Takes the Box's turn, or `None` when this thread holds it already, as a resume honouring a
/// recorded forced close does; under `force`, a start in flight is stopped and the close goes first.
///
/// The stopped start records its setup as failed before it lets go. One that has not stopped
/// after [`INTERRUPT_WAIT`] is refused, and the Box is left as it is. A start still acquiring its
/// machine has that boot cancelled, and the close is wanted first so a restart honours it.
pub(super) fn turn<'a>(
    cx: &'a Cx,
    row: &BoxRow,
    force: bool,
) -> Result<Option<Turn<'a>>, CommandError> {
    if cx.owners().holds(row.id) {
        return Ok(None);
    }
    let name = row.name.as_str();
    let starting = matches!(row.effect, Some(Effect::Setup | Effect::Acquire));
    let acquiring = row.effect == Some(Effect::Acquire);
    let store = cx.store()?;
    let deadline = cx.clock().instant() + QUEUE_WAIT;
    let cmd = Cmd::Close { force };
    let stop = || {
        if acquiring {
            let closing = Wanted::Close {
                force: true,
                reason: None,
            };
            store.apply(Decision::new(row.id).want(WantChange::Raise(closing)))?;
        }
        cancel_boot(cx, store, row)
    };
    match cx
        .owners()
        .preempting(row.id, cmd, cx.env().cancel(), deadline, stop)
    {
        Ok((turn, _)) => Ok(Some(turn)),
        Err(e) if e.kind == ErrorKind::Refused && starting && !force => Err(e.with_hint(format!(
            "`moor close {name} --force` interrupts the start and closes the Box"
        ))),
        Err(e) if e.kind == ErrorKind::Refused && force && starting => {
            if acquiring {
                let done = WantChange::Done(WantedKind::Close);
                store.apply(Decision::new(row.id).want(done))?;
            }
            Err(CommandError::new(
                ErrorKind::Refused,
                format!(
                    "box busy: the start of {name} did not stop within {}s of the forced close; \
                     the Box was left as it is",
                    INTERRUPT_WAIT.as_secs()
                ),
            ))
        }
        Err(e) => Err(e),
    }
}
