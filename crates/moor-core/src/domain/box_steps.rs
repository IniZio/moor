//! Translates a Task input into steps of the Box lifecycle and its primary Session.
//!
//! Callers still send [`TaskInput`] and read a combined [`TaskState`]; the Box and the Session
//! move separately, each only through its own `step`.

use super::box_state::{BoxLifecycle, BoxLifecycleInput, StepError};
use super::session_state::{SessionInput, SessionState};
use super::task::{TaskInput, TaskState};
use super::Illegal;
use TaskInput as I;

/// Box and primary Session states of a freshly inserted Box.
///
/// # Errors
/// Returns [`Illegal`] with `from` = `none` for any input but `Insert`.
pub fn initial(input: &TaskInput) -> Result<(BoxLifecycle, SessionState), Illegal> {
    TaskState::initial(input)?;
    let lifecycle = BoxLifecycle::initial(&BoxLifecycleInput::Insert)?;
    let session = SessionState::initial(&SessionInput::Open)?;
    Ok((lifecycle, session))
}

/// The combined state callers still read.
#[must_use]
pub const fn summary(lifecycle: BoxLifecycle, primary: SessionState) -> TaskState {
    match lifecycle {
        BoxLifecycle::Checking => TaskState::Checking,
        BoxLifecycle::Rejected => TaskState::Rejected,
        BoxLifecycle::WaitingForBox => TaskState::WaitingForBox,
        BoxLifecycle::SettingUp => TaskState::SettingUp,
        BoxLifecycle::SetupFailed => TaskState::SetupFailed,
        BoxLifecycle::Ready => match primary {
            SessionState::Idle | SessionState::Ended => TaskState::Idle,
            _ => TaskState::Working,
        },
        BoxLifecycle::Asleep => TaskState::Asleep,
        BoxLifecycle::Closed => TaskState::Closed,
    }
}

/// Step the Box and its primary Session for one input; `others` are the Box's other Sessions.
///
/// # Errors
/// Returns [`StepError::Illegal`] for a pair outside the tables, named as the combined state
/// would name it, and [`StepError::SleepBlocked`] when another Session is not Idle.
pub fn step(
    lifecycle: BoxLifecycle,
    primary: SessionState,
    others: &[SessionState],
    input: &TaskInput,
) -> Result<(BoxLifecycle, SessionState), StepError> {
    let cx = Cx {
        lifecycle,
        primary,
        others,
        input,
    };
    match input {
        I::Insert => Err(cx.illegal()),
        I::Rejected { reason } => cx.box_only(&BoxLifecycleInput::Rejected {
            reason: reason.clone(),
        }),
        I::AtCapacity { max_boxes } => cx.box_only(&BoxLifecycleInput::AtCapacity {
            max_boxes: *max_boxes,
        }),
        I::Admitted => cx.box_only(&BoxLifecycleInput::Admitted),
        I::Promoted => cx.box_only(&BoxLifecycleInput::Promoted),
        I::Retry => cx.box_only(&BoxLifecycleInput::Retry),
        I::BoxReady { setup } => {
            let lc = cx.boxed(
                lifecycle,
                &BoxLifecycleInput::Ready {
                    setup: setup.clone(),
                },
            )?;
            Ok((lc, cx.sess(primary, &SessionInput::Started)?))
        }
        I::AcquireFailed { cause } => cx.setup_failed(cause, None),
        I::CredFailed { which, error } => cx.setup_failed(error, Some(which)),
        I::DotfilesFailed { error } | I::SyncFailed { error } => cx.setup_failed(error, None),
        I::AgentQuiet => cx.ready_only(&SessionInput::AgentQuiet),
        I::AgentBusy => cx.ready_only(&SessionInput::AgentBusy),
        I::RunStarted => cx.ready_only(&SessionInput::RunStarted),
        I::SleepDue | I::SleepCmd | I::Wake | I::Close => cx.lifecycle_change(),
    }
}

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

/// One step's inputs, with the error mapping shared by every arm.
struct Cx<'a> {
    /// The Box before the step.
    lifecycle: BoxLifecycle,
    /// The primary Session before the step.
    primary: SessionState,
    /// The Box's other Sessions.
    others: &'a [SessionState],
    /// The input being applied.
    input: &'a TaskInput,
}

/// The Box and primary Session after a step.
type Parts = Result<(BoxLifecycle, SessionState), StepError>;

impl Cx<'_> {
    /// The error text the combined state would give.
    fn illegal(&self) -> StepError {
        StepError::Illegal(Illegal {
            from: summary(self.lifecycle, self.primary).as_str().to_owned(),
            input: self.input.name().to_owned(),
        })
    }

    /// Steps the Box, naming a refusal as the combined state would.
    fn boxed(&self, lc: BoxLifecycle, i: &BoxLifecycleInput) -> Result<BoxLifecycle, StepError> {
        match lc.step(&[], i) {
            Err(StepError::Illegal(_)) => Err(self.illegal()),
            other => other,
        }
    }

    /// Steps a Session, naming a refusal as the combined state would.
    fn sess(&self, s: SessionState, i: &SessionInput) -> Result<SessionState, StepError> {
        s.step(i).map_err(|_| self.illegal())
    }

    /// Steps the primary Session; an Ended one stays Ended, as it takes no further input.
    fn primary_after(&self, i: &SessionInput) -> Result<SessionState, StepError> {
        if self.primary == SessionState::Ended {
            Ok(self.primary)
        } else {
            self.sess(self.primary, i)
        }
    }

    /// A Box-only input leaves the primary Session as it was.
    fn box_only(&self, i: &BoxLifecycleInput) -> Parts {
        Ok((self.boxed(self.lifecycle, i)?, self.primary))
    }

    /// Fails setup with a cause and, for credentials, which one.
    fn setup_failed(&self, cause: &str, which: Option<&String>) -> Parts {
        self.box_only(&BoxLifecycleInput::SetupFailed {
            cause: cause.to_owned(),
            which: which.cloned(),
        })
    }

    /// A Session input, legal only while the Box is Ready.
    fn ready_only(&self, i: &SessionInput) -> Parts {
        if self.lifecycle == BoxLifecycle::Ready {
            Ok((self.lifecycle, self.primary_after(i)?))
        } else {
            Err(self.illegal())
        }
    }

    /// Sleeps the Box with the primary and the other Sessions.
    fn sleep(&self, primary: SessionState, auto: bool) -> Parts {
        let mut sessions = vec![primary];
        sessions.extend_from_slice(self.others);
        let lc = match self
            .lifecycle
            .step(&sessions, &BoxLifecycleInput::Sleep { auto })
        {
            Err(StepError::Illegal(_)) => Err(self.illegal()),
            other => other,
        }?;
        Ok((lc, primary))
    }

    /// Sleep, wake and close, which move the Box and then the Session.
    fn lifecycle_change(&self) -> Parts {
        let ready = self.lifecycle == BoxLifecycle::Ready;
        match self.input {
            I::SleepDue
                if ready && !matches!(self.primary, SessionState::Idle | SessionState::Ended) =>
            {
                Err(self.illegal())
            }

            I::SleepCmd if ready && self.primary == SessionState::Working => {
                self.sleep(self.sess(self.primary, &SessionInput::AgentQuiet)?, false)
            }
            I::SleepDue => self.sleep(self.primary, true),
            I::SleepCmd => self.sleep(self.primary, false),
            I::Wake => {
                let lc = self.boxed(self.lifecycle, &BoxLifecycleInput::Wake)?;
                Ok((lc, self.primary_after(&SessionInput::AgentBusy)?))
            }
            _ => {
                let lc = self.boxed(self.lifecycle, &BoxLifecycleInput::Close)?;
                let session = if self.primary == SessionState::Ended {
                    self.primary
                } else {
                    self.sess(self.primary, &SessionInput::End)?
                };
                Ok((lc, session))
            }
        }
    }
}

#[cfg(test)]
mod tests;
