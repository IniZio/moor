//! Waits for the guest of a machine that just came up, before an op acts on it.

use std::time::Duration;

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::{BoxHandle, BoxId, BoxPath, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::Store;

/// A file any guest can read, or lack, so reading it shows only that the guest answers.
const GUEST_PROBE: &str = "/proc/self/environ";
/// Longest a wait for the guest lasts before the op goes on without it.
const GUEST_WAIT: Duration = Duration::from_mins(2);
/// Pause between looks at the guest.
const GUEST_POLL: Duration = Duration::from_secs(1);

/// Waits until the guest behind `handle` answers. A machine that was just acquired, forked or
/// resumed is running before its sshd answers, and on a busy node its first ssh can outlast an
/// op's own short deadline. Past `GUEST_WAIT` an `agent_unreachable` event on Box `id` says so,
/// and the op's own call fails. A forced close or a drain ends the wait early.
pub(crate) fn await_guest(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    handle: &BoxHandle,
) -> Result<(), CommandError> {
    let probe =
        BoxPath::restore(GUEST_PROBE).map_err(|e| CommandError::new(ErrorKind::Internal, e))?;
    let bound = i64::try_from(GUEST_WAIT.as_millis()).unwrap_or(i64::MAX);
    let from = cx.clock().now().as_millis();
    let mut attempts = 0;
    loop {
        attempts += 1;
        let Err(e) = cx.driver().peek_file(handle, &probe) else {
            return Ok(());
        };
        if cx.env().cancel().is_cancelled() || cx.env().draining().is_cancelled() {
            return Ok(());
        }
        if cx.clock().now().as_millis().saturating_sub(from) >= bound {
            let error = format!("no answer within {}s: {e}", GUEST_WAIT.as_secs());
            let event = ExtraEvent::AgentUnreachable { attempts, error };
            store.apply(Decision::new(id).event(event))?;
            return Ok(());
        }
        cx.clock().sleep(GUEST_POLL);
    }
}
