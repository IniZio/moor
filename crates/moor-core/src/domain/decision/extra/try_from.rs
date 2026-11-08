//! The stored event a caller may pass again, read back as an [`ExtraEvent`].

use super::ExtraEvent;
use crate::domain::event::{
    AccessEvent, Event, NodeDiskEvent, ProfileEvent, StallEvent, TimingEvent,
};

impl TryFrom<Event> for ExtraEvent {
    type Error = Event;

    /// Returns the original event when its kind is derived.
    #[allow(clippy::too_many_lines, reason = "one arm per event variant")]
    fn try_from(e: Event) -> Result<Self, Event> {
        match e {
            Event::StaysUp { reason } => Ok(Self::StaysUp { reason }),
            Event::DiskLost => Ok(Self::DiskLost),
            Event::CachesNotSaved => Ok(Self::CachesNotSaved),
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
            Event::Access(AccessEvent::Copied { from, to }) => Ok(Self::Copied { from, to }),
            Event::Access(AccessEvent::ExecRan {
                command,
                exit,
                timed_out,
            }) => Ok(Self::ExecRan {
                command,
                exit,
                timed_out,
            }),
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
            Event::Timing(TimingEvent::SetupLineStarted { index, total, line }) => {
                Ok(Self::SetupLineStarted { index, total, line })
            }
            Event::Timing(TimingEvent::SetupLineFinished {
                index,
                total,
                ms,
                ok,
            }) => Ok(Self::SetupLineFinished {
                index,
                total,
                ms,
                ok,
            }),
            Event::Timing(TimingEvent::WarmStarted {
                layer,
                layers,
                saved_ms,
                fork_ms,
                steps,
                rerun_from,
                finished,
            }) => Ok(Self::WarmStarted {
                layer,
                layers,
                saved_ms,
                fork_ms,
                steps,
                rerun_from,
                finished,
            }),
            Event::Timing(TimingEvent::WarmRerun {
                commit,
                changed,
                ignored,
                lines_kept,
            }) => Ok(Self::WarmRerun {
                commit,
                changed,
                ignored,
                lines_kept,
            }),
            Event::Timing(TimingEvent::WarmRerunFailed { line, tail }) => {
                Ok(Self::WarmRerunFailed { line, tail })
            }
            Event::Timing(TimingEvent::ColdAsked) => Ok(Self::ColdAsked),
            Event::Timing(TimingEvent::WarmStartFailed { layer, error }) => {
                Ok(Self::WarmStartFailed { layer, error })
            }
            Event::Timing(TimingEvent::CacheCold { reason, detail }) => {
                Ok(Self::CacheCold { reason, detail })
            }
            Event::Timing(TimingEvent::CacheEmptied { volume, reason }) => {
                Ok(Self::CacheEmptied { volume, reason })
            }
            Event::Timing(TimingEvent::CheckpointTaken {
                layer,
                key,
                checkpoint,
                duration_ms,
                error,
            }) => Ok(Self::CheckpointTaken {
                layer,
                key,
                checkpoint,
                duration_ms,
                error,
            }),
            Event::Timing(TimingEvent::CheckpointEvicted {
                checkpoint,
                repo,
                bytes,
                reason,
                free_mib,
                needs_mib,
            }) => Ok(Self::CheckpointEvicted {
                checkpoint,
                repo,
                bytes,
                reason,
                free_mib,
                needs_mib,
            }),
            Event::Profile(ProfileEvent::ProfileResolved {
                repo,
                sha,
                sources,
                unresolved,
            }) => Ok(Self::ProfileResolved {
                repo,
                sha,
                sources,
                unresolved,
            }),
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
            Event::Stall(StallEvent::AgentStalled { doing, quiet_secs }) => {
                Ok(Self::AgentStalled { doing, quiet_secs })
            }
            Event::Stall(StallEvent::AgentResumed { quiet_secs }) => {
                Ok(Self::AgentResumed { quiet_secs })
            }
            Event::NodeDisk(NodeDiskEvent::NodeDiskLow {
                node,
                free_mib,
                total_mib,
            }) => Ok(Self::NodeDiskLow {
                node,
                free_mib,
                total_mib,
            }),
            Event::NodeDisk(NodeDiskEvent::NodeDiskRecovered {
                node,
                free_mib,
                total_mib,
            }) => Ok(Self::NodeDiskRecovered {
                node,
                free_mib,
                total_mib,
            }),
            Event::Stall(StallEvent::PromptNotGiven { attempt, error }) => {
                Ok(Self::PromptNotGiven { attempt, error })
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
            Event::TranscriptFailed {
                trigger,
                error,
                unlinked,
            } => Ok(Self::TranscriptFailed {
                trigger,
                error,
                unlinked,
            }),
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
