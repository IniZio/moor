//! Conversion between a decision's events and stored events.

use super::ExtraEvent;
use crate::domain::event::{Event, ProfileEvent, TimingEvent};

impl From<ExtraEvent> for Event {
    #[allow(clippy::too_many_lines, reason = "one arm per event variant")]
    fn from(e: ExtraEvent) -> Self {
        match e {
            ExtraEvent::StaysUp { reason } => Self::StaysUp { reason },
            ExtraEvent::Ran {
                run,
                exit,
                interrupted,
                error,
            } => Self::Ran {
                run,
                exit,
                interrupted,
                error,
            },
            ExtraEvent::Copied { from, to } => Self::Copied { from, to },
            ExtraEvent::SessionStarted { session } => Self::SessionStarted { session },
            ExtraEvent::SessionFailed { step, message } => Self::SessionFailed { step, message },
            ExtraEvent::LaunchTimedOut { after_secs } => Self::LaunchTimedOut { after_secs },
            ExtraEvent::RelaunchCapped { count, window_secs } => {
                Self::RelaunchCapped { count, window_secs }
            }
            ExtraEvent::Relaunched {
                old_pane,
                new_pane,
                prompt,
                cause,
            } => Self::Relaunched {
                old_pane,
                new_pane,
                prompt,
                cause,
            },
            ExtraEvent::SetupFailed { line } => Self::SetupFailed { line },
            ExtraEvent::SetupFinished { ms, ok } => {
                Self::Timing(TimingEvent::SetupFinished { ms, ok })
            }
            ExtraEvent::Cloned { ms } => Self::Timing(TimingEvent::Cloned { ms }),
            ExtraEvent::StepStarted { step } => Self::Timing(TimingEvent::StepStarted { step }),
            ExtraEvent::StepFinished { step, ms, ok } => {
                Self::Timing(TimingEvent::StepFinished { step, ms, ok })
            }
            ExtraEvent::ProfileResolved { repo, sha } => {
                Self::Profile(ProfileEvent::ProfileResolved { repo, sha })
            }
            ExtraEvent::ProfileFailed { step, detail } => {
                Self::Profile(ProfileEvent::ProfileFailed { step, detail })
            }
            ExtraEvent::Checked { credentials } => Self::Checked { credentials },
            ExtraEvent::RunStarted { run } => Self::RunStarted { run },
            ExtraEvent::Attached => Self::Attached,
            ExtraEvent::SessionsStopped { sessions } => Self::SessionsStopped { sessions },
            ExtraEvent::AgentPromptBlocked { pane, prompt, line } => {
                Self::AgentPromptBlocked { pane, prompt, line }
            }
            ExtraEvent::AgentSaid { text } => Self::AgentSaid { text },
            ExtraEvent::AgentDoing { text } => Self::AgentDoing { text },
            ExtraEvent::BoxProgress { text } => Self::BoxProgress { text },
            ExtraEvent::NeedsInput { question, options } => Self::NeedsInput { question, options },
            ExtraEvent::AgentUnreachable { attempts, error } => {
                Self::AgentUnreachable { attempts, error }
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
            ExtraEvent::TranscriptSkipped { trigger, reason } => {
                Self::TranscriptSkipped { trigger, reason }
            }
            ExtraEvent::Steered {
                session,
                message,
                submit_retried,
                hook_before,
                new_turn,
            } => Self::Steered {
                session,
                message,
                submit_retried,
                hook_before,
                new_turn,
            },
            ExtraEvent::TurnDone { steer, outcome } => Self::TurnDone { steer, outcome },
            ExtraEvent::ProxyStarted {
                box_port,
                pid,
                handle,
            } => Self::ProxyStarted {
                box_port,
                pid,
                handle,
            },
            ExtraEvent::TunnelRestarted { box_port, reason } => {
                Self::TunnelRestarted { box_port, reason }
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
                error,
            } => Ok(Self::Ran {
                run,
                exit,
                interrupted,
                error,
            }),
            Event::Copied { from, to } => Ok(Self::Copied { from, to }),
            Event::SessionStarted { session } => Ok(Self::SessionStarted { session }),
            Event::SessionFailed { step, message } => Ok(Self::SessionFailed { step, message }),
            Event::LaunchTimedOut { after_secs } => Ok(Self::LaunchTimedOut { after_secs }),
            Event::RelaunchCapped { count, window_secs } => {
                Ok(Self::RelaunchCapped { count, window_secs })
            }
            Event::Relaunched {
                old_pane,
                new_pane,
                prompt,
                cause,
            } => Ok(Self::Relaunched {
                old_pane,
                new_pane,
                prompt,
                cause,
            }),
            Event::SetupFailed { line } => Ok(Self::SetupFailed { line }),
            Event::Timing(TimingEvent::SetupFinished { ms, ok }) => {
                Ok(Self::SetupFinished { ms, ok })
            }
            Event::Timing(TimingEvent::Cloned { ms }) => Ok(Self::Cloned { ms }),
            Event::Timing(TimingEvent::StepStarted { step }) => Ok(Self::StepStarted { step }),
            Event::Timing(TimingEvent::StepFinished { step, ms, ok }) => {
                Ok(Self::StepFinished { step, ms, ok })
            }
            Event::Profile(ProfileEvent::ProfileResolved { repo, sha }) => {
                Ok(Self::ProfileResolved { repo, sha })
            }
            Event::Profile(ProfileEvent::ProfileFailed { step, detail }) => {
                Ok(Self::ProfileFailed { step, detail })
            }
            Event::Checked { credentials } => Ok(Self::Checked { credentials }),
            Event::RunStarted { run } => Ok(Self::RunStarted { run }),
            Event::Attached => Ok(Self::Attached),
            Event::SessionsStopped { sessions } => Ok(Self::SessionsStopped { sessions }),
            Event::AgentPromptBlocked { pane, prompt, line } => {
                Ok(Self::AgentPromptBlocked { pane, prompt, line })
            }
            Event::AgentSaid { text } => Ok(Self::AgentSaid { text }),
            Event::AgentDoing { text } => Ok(Self::AgentDoing { text }),
            Event::BoxProgress { text } => Ok(Self::BoxProgress { text }),
            Event::NeedsInput { question, options } => Ok(Self::NeedsInput { question, options }),
            Event::AgentUnreachable { attempts, error } => {
                Ok(Self::AgentUnreachable { attempts, error })
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
            Event::TranscriptSkipped { trigger, reason } => {
                Ok(Self::TranscriptSkipped { trigger, reason })
            }
            Event::Steered {
                session,
                message,
                submit_retried,
                hook_before,
                new_turn,
            } => Ok(Self::Steered {
                session,
                message,
                submit_retried,
                hook_before,
                new_turn,
            }),
            Event::TurnDone { steer, outcome } => Ok(Self::TurnDone { steer, outcome }),
            Event::ProxyStarted {
                box_port,
                pid,
                handle,
            } => Ok(Self::ProxyStarted {
                box_port,
                pid,
                handle,
            }),
            Event::TunnelRestarted { box_port, reason } => {
                Ok(Self::TunnelRestarted { box_port, reason })
            }
            e @ (Event::Started
            | Event::Rejected { .. }
            | Event::Pending { .. }
            | Event::SettingUp { .. }
            | Event::Working
            | Event::CredentialFailed { .. }
            | Event::EgressRefused { .. }
            | Event::EgressAborted { .. }
            | Event::CredentialWithheld { .. }
            | Event::ProxyCredentialFailed { .. }
            | Event::AgentRateLimited { .. }
            | Event::ProxyCredentialRejected { .. }
            | Event::ProxyCredentialRecovered { .. }
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
            | Event::BoxReleaseFailed { .. }
            | Event::ArtifactPublished { .. }
            | Event::SessionEnded { .. }
            | Event::AgentLinkLost { .. }
            | Event::AgentLinkRestored { .. }
            | Event::Machine(_)) => Err(e),
        }
    }
}
