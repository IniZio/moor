//! Events a decision may carry, and their conversion to and from stored events.

use crate::domain::event::{CheckedCred, Event};
use crate::name::RunId;

/// An event a caller may pass; every other kind is derived from state steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtraEvent {
    /// The driver refused to sleep an idle task's box, so it keeps running.
    StaysUp {
        /// The driver's reason.
        reason: String,
    },
    /// A command run finished.
    Ran {
        /// The run id; none when the run was refused before a run row existed.
        run: Option<RunId>,
        /// The exit code, when known.
        exit: Option<i32>,
        /// True when the run was interrupted.
        interrupted: bool,
    },
    /// Files were copied.
    Copied {
        /// The copy source.
        from: String,
        /// The copy destination.
        to: String,
    },
    /// An agent session started.
    SessionStarted {
        /// The session id.
        session: String,
    },
    /// The agent launch failed; the Task stays in its state, so no step derives it.
    SessionFailed {
        /// The launch step that failed.
        step: String,
        /// What went wrong.
        message: String,
    },
    /// The agent was started again after its pane ended.
    Relaunched {
        /// The pane of the session that ended.
        old_pane: Option<String>,
        /// The pane of the new session.
        new_pane: Option<String>,
        /// The goal the new session works on.
        goal: Option<String>,
    },
    /// Setup failed on a Task that stays in its state, so no step derives it.
    SetupFailed {
        /// The failing output line.
        line: String,
    },
    /// Admission checked credentials.
    Checked {
        /// The credentials that were checked.
        credentials: Vec<CheckedCred>,
    },
    /// A command run started.
    RunStarted {
        /// The run id.
        run: RunId,
    },
    /// A start attached to an existing open task.
    Attached,
    /// Close ended the live agent sessions.
    SessionsStopped {
        /// The ended session ids.
        sessions: Vec<String>,
    },
    /// The agent waits at a first-run prompt.
    AgentPromptBlocked {
        /// The agent's pane.
        pane: String,
        /// The prompt, by name.
        prompt: String,
        /// The pane line that showed it.
        line: String,
    },
    /// Reading the agent's pane for first-run prompts failed.
    AgentPromptCheckFailed {
        /// The agent's pane.
        pane: String,
        /// What went wrong.
        error: String,
    },
    /// The agent's transcript was saved on the host.
    TranscriptSaved {
        /// What ended the session: `relaunch` or `close`.
        trigger: String,
        /// The host path of the archive.
        path: String,
        /// The digest of the archive.
        digest: String,
    },
    /// The agent's transcript could not be saved.
    TranscriptFailed {
        /// What ended the session: `relaunch` or `close`.
        trigger: String,
        /// What went wrong.
        error: String,
    },
}

impl From<ExtraEvent> for Event {
    fn from(e: ExtraEvent) -> Self {
        match e {
            ExtraEvent::StaysUp { reason } => Self::StaysUp { reason },
            ExtraEvent::Ran {
                run,
                exit,
                interrupted,
            } => Self::Ran {
                run,
                exit,
                interrupted,
            },
            ExtraEvent::Copied { from, to } => Self::Copied { from, to },
            ExtraEvent::SessionStarted { session } => Self::SessionStarted { session },
            ExtraEvent::SessionFailed { step, message } => Self::SessionFailed { step, message },
            ExtraEvent::Relaunched {
                old_pane,
                new_pane,
                goal,
            } => Self::Relaunched {
                old_pane,
                new_pane,
                goal,
            },
            ExtraEvent::SetupFailed { line } => Self::SetupFailed { line },
            ExtraEvent::Checked { credentials } => Self::Checked { credentials },
            ExtraEvent::RunStarted { run } => Self::RunStarted { run },
            ExtraEvent::Attached => Self::Attached,
            ExtraEvent::SessionsStopped { sessions } => Self::SessionsStopped { sessions },
            ExtraEvent::AgentPromptBlocked { pane, prompt, line } => {
                Self::AgentPromptBlocked { pane, prompt, line }
            }
            ExtraEvent::AgentPromptCheckFailed { pane, error } => {
                Self::AgentPromptCheckFailed { pane, error }
            }
            ExtraEvent::TranscriptSaved {
                trigger,
                path,
                digest,
            } => Self::TranscriptSaved {
                trigger,
                path,
                digest,
            },
            ExtraEvent::TranscriptFailed { trigger, error } => {
                Self::TranscriptFailed { trigger, error }
            }
        }
    }
}

impl TryFrom<Event> for ExtraEvent {
    type Error = Event;

    /// Returns the original event when its kind is derived.
    #[allow(clippy::too_many_lines, reason = "one arm per event variant")]
    fn try_from(e: Event) -> Result<Self, Event> {
        match e {
            Event::StaysUp { reason } => Ok(Self::StaysUp { reason }),
            Event::Ran {
                run,
                exit,
                interrupted,
            } => Ok(Self::Ran {
                run,
                exit,
                interrupted,
            }),
            Event::Copied { from, to } => Ok(Self::Copied { from, to }),
            Event::SessionStarted { session } => Ok(Self::SessionStarted { session }),
            Event::SessionFailed { step, message } => Ok(Self::SessionFailed { step, message }),
            Event::Relaunched {
                old_pane,
                new_pane,
                goal,
            } => Ok(Self::Relaunched {
                old_pane,
                new_pane,
                goal,
            }),
            Event::SetupFailed { line } => Ok(Self::SetupFailed { line }),
            Event::Checked { credentials } => Ok(Self::Checked { credentials }),
            Event::RunStarted { run } => Ok(Self::RunStarted { run }),
            Event::Attached => Ok(Self::Attached),
            Event::SessionsStopped { sessions } => Ok(Self::SessionsStopped { sessions }),
            Event::AgentPromptBlocked { pane, prompt, line } => {
                Ok(Self::AgentPromptBlocked { pane, prompt, line })
            }
            Event::AgentPromptCheckFailed { pane, error } => {
                Ok(Self::AgentPromptCheckFailed { pane, error })
            }
            Event::TranscriptSaved {
                trigger,
                path,
                digest,
            } => Ok(Self::TranscriptSaved {
                trigger,
                path,
                digest,
            }),
            Event::TranscriptFailed { trigger, error } => {
                Ok(Self::TranscriptFailed { trigger, error })
            }
            e @ (Event::Started
            | Event::Rejected { .. }
            | Event::WaitingForBox { .. }
            | Event::SettingUp { .. }
            | Event::Working
            | Event::CredentialFailed { .. }
            | Event::Idled
            | Event::Busy
            | Event::Asleep { .. }
            | Event::Woke
            | Event::Closed { .. }
            | Event::BoxRequested
            | Event::BoxReady
            | Event::BoxFailed { .. }
            | Event::BoxAsleep
            | Event::BoxGone
            | Event::BoxDeleted
            | Event::BoxReleaseFailed { .. }) => Err(e),
        }
    }
}
