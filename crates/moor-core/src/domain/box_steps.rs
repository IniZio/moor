//! Steps the Box lifecycle and its primary Session together; each moves only through its own
//! `step`.

use super::box_state::{BoxLifecycle, BoxLifecycleInput, StepError};
use super::session_state::{SessionInput, SessionState};
use super::Illegal;

/// Step the Box and its primary Session by separate inputs; `others` are the Box's other Sessions.
///
/// The Box steps first, with the primary and `others` as the Sessions it checks. An automatic
/// sleep from Ready is refused as illegal while the primary is not quiet; a commanded sleep from
/// Ready first quiets a Working primary. The Session input then steps the primary. A Session
/// input alone is legal only while the Box is Ready. An Ended primary stays Ended.
///
/// # Errors
/// Returns [`StepError::Illegal`] naming the machine that refused, and
/// [`StepError::SleepBlocked`] when another Session is not quiet.
pub fn apply(
    lifecycle: BoxLifecycle,
    primary: SessionState,
    others: &[SessionState],
    lifecycle_input: Option<&BoxLifecycleInput>,
    session_input: Option<&SessionInput>,
) -> Result<(BoxLifecycle, SessionState), StepError> {
    let Some(l) = lifecycle_input else {
        let Some(s) = session_input else {
            return Ok((lifecycle, primary));
        };
        if lifecycle != BoxLifecycle::Ready {
            return Err(StepError::Illegal(Illegal {
                from: lifecycle.as_str().to_owned(),
                input: s.name().to_owned(),
            }));
        }
        return Ok((lifecycle, primary_step(primary, s)?));
    };
    let ready = lifecycle == BoxLifecycle::Ready;
    let mut primary = primary;
    match l {
        BoxLifecycleInput::Sleep { auto: true } if ready && !primary.is_quiet() => {
            return Err(StepError::Illegal(Illegal {
                from: lifecycle.as_str().to_owned(),
                input: l.name().to_owned(),
            }));
        }
        BoxLifecycleInput::Sleep { auto: false } if ready && primary == SessionState::Working => {
            primary = primary.step(&SessionInput::AgentQuiet)?;
        }
        _ => {}
    }
    let mut sessions = vec![primary];
    sessions.extend_from_slice(others);
    let lc = lifecycle.step(&sessions, l)?;
    match session_input {
        Some(s) => Ok((lc, primary_step(primary, s)?)),
        None => Ok((lc, primary)),
    }
}

/// Steps the primary Session; an Ended one stays Ended, as it takes no further input.
fn primary_step(primary: SessionState, i: &SessionInput) -> Result<SessionState, StepError> {
    if primary == SessionState::Ended {
        Ok(primary)
    } else {
        Ok(primary.step(i)?)
    }
}

#[cfg(test)]
mod tests;
