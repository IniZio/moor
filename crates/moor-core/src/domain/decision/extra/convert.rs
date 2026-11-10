//! Conversion of a decision's events to stored events.

use super::ExtraEvent;
use crate::domain::event::{
    AccessEvent, CloseEvent, Event, NodeDiskEvent, ProfileEvent, StallEvent, TimingEvent,
    WorkflowEvent,
};

impl From<ExtraEvent> for Event {
    #[allow(clippy::too_many_lines, reason = "one arm per event variant")]
    fn from(e: ExtraEvent) -> Self {
        match e {
            ExtraEvent::StaysUp { reason } => Self::StaysUp { reason },
            ExtraEvent::DiskLost => Self::DiskLost,
            ExtraEvent::CachesNotSaved => Self::Close(CloseEvent::CachesNotSaved),
            ExtraEvent::CloseRefused {
                reason,
                error,
                slept,
            } => Self::Close(CloseEvent::CloseRefused {
                reason,
                error,
                slept,
            }),
            ExtraEvent::WantedDropped => Self::WantedDropped,
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
            ExtraEvent::Copied { from, to } => Self::Access(AccessEvent::Copied { from, to }),
            ExtraEvent::ExecRan {
                command,
                exit,
                timed_out,
            } => Self::Access(AccessEvent::ExecRan {
                command,
                exit,
                timed_out,
            }),
            ExtraEvent::SessionStarted { session } => Self::SessionStarted { session },
            ExtraEvent::SessionFailed { step, message } => Self::SessionFailed { step, message },
            ExtraEvent::LaunchTimedOut { after_secs } => Self::LaunchTimedOut { after_secs },
            ExtraEvent::RelaunchCapped { count, window_secs } => {
                Self::RelaunchCapped { count, window_secs }
            }
            ExtraEvent::Relaunched { prompt, cause } => Self::Relaunched { prompt, cause },
            ExtraEvent::SetupFailed { line } => Self::SetupFailed { line, why: None },
            ExtraEvent::SetupFinished { ms, ok } => {
                Self::Timing(TimingEvent::SetupFinished { ms, ok })
            }
            ExtraEvent::Cloned { ms } => Self::Timing(TimingEvent::Cloned { ms }),
            ExtraEvent::StepStarted { step } => Self::Timing(TimingEvent::StepStarted { step }),
            ExtraEvent::StepFinished { step, ms, ok } => {
                Self::Timing(TimingEvent::StepFinished { step, ms, ok })
            }
            ExtraEvent::SetupLineStarted { index, total, line } => {
                Self::Timing(TimingEvent::SetupLineStarted { index, total, line })
            }
            ExtraEvent::SetupLineFinished {
                index,
                total,
                ms,
                ok,
            } => Self::Timing(TimingEvent::SetupLineFinished {
                index,
                total,
                ms,
                ok,
            }),
            ExtraEvent::WarmStarted {
                layer,
                layers,
                saved_ms,
                fork_ms,
                steps,
                rerun_from,
                finished,
            } => Self::Timing(TimingEvent::WarmStarted {
                layer,
                layers,
                saved_ms,
                fork_ms,
                steps,
                rerun_from,
                finished,
            }),
            ExtraEvent::WarmRerun {
                commit,
                changed,
                ignored,
                lines_kept,
            } => Self::Timing(TimingEvent::WarmRerun {
                commit,
                changed,
                ignored,
                lines_kept,
            }),
            ExtraEvent::WarmRerunFailed { line, tail } => {
                Self::Timing(TimingEvent::WarmRerunFailed { line, tail })
            }
            ExtraEvent::ColdAsked => Self::Timing(TimingEvent::ColdAsked),
            ExtraEvent::WarmStartFailed { layer, error } => {
                Self::Timing(TimingEvent::WarmStartFailed { layer, error })
            }
            ExtraEvent::CacheCold { reason, detail } => {
                Self::Timing(TimingEvent::CacheCold { reason, detail })
            }
            ExtraEvent::CacheEmptied { volume, reason } => {
                Self::Timing(TimingEvent::CacheEmptied { volume, reason })
            }
            ExtraEvent::CheckpointTaken {
                layer,
                key,
                checkpoint,
                duration_ms,
                error,
            } => Self::Timing(TimingEvent::CheckpointTaken {
                layer,
                key,
                checkpoint,
                duration_ms,
                error,
            }),
            ExtraEvent::CheckpointEvicted {
                checkpoint,
                repo,
                bytes,
                reason,
                free_mib,
                needs_mib,
            } => Self::Timing(TimingEvent::CheckpointEvicted {
                checkpoint,
                repo,
                bytes,
                reason,
                free_mib,
                needs_mib,
            }),
            ExtraEvent::ProfileResolved {
                repo,
                sha,
                sources,
                unresolved,
            } => Self::Profile(ProfileEvent::ProfileResolved {
                repo,
                sha,
                sources,
                unresolved,
            }),
            ExtraEvent::ProfileFailed { step, detail } => {
                Self::Profile(ProfileEvent::ProfileFailed { step, detail })
            }
            ExtraEvent::Checked { credentials } => Self::Checked { credentials },
            ExtraEvent::RunStarted { run } => Self::RunStarted { run },
            ExtraEvent::Attached => Self::Attached,
            ExtraEvent::SessionsStopped { sessions } => Self::SessionsStopped { sessions },
            ExtraEvent::AgentPromptBlocked { prompt, line } => {
                Self::AgentPromptBlocked { prompt, line }
            }
            ExtraEvent::AgentSaid { text } => Self::AgentSaid { text },
            ExtraEvent::AgentDoing { text } => Self::AgentDoing { text },
            ExtraEvent::BoxProgress { text } => Self::BoxProgress { text },
            ExtraEvent::NeedsInput { question, options } => Self::NeedsInput { question, options },
            ExtraEvent::AgentUnreachable { attempts, error } => {
                Self::AgentUnreachable { attempts, error }
            }
            ExtraEvent::AgentStalled { doing, quiet_secs } => {
                Self::Stall(StallEvent::AgentStalled { doing, quiet_secs })
            }
            ExtraEvent::AgentResumed { quiet_secs } => {
                Self::Stall(StallEvent::AgentResumed { quiet_secs })
            }
            ExtraEvent::NodeDiskLow {
                node,
                free_mib,
                total_mib,
            } => Self::NodeDisk(NodeDiskEvent::NodeDiskLow {
                node,
                free_mib,
                total_mib,
            }),
            ExtraEvent::NodeDiskRecovered {
                node,
                free_mib,
                total_mib,
            } => Self::NodeDisk(NodeDiskEvent::NodeDiskRecovered {
                node,
                free_mib,
                total_mib,
            }),
            ExtraEvent::PromptNotGiven { attempt, error } => {
                Self::Stall(StallEvent::PromptNotGiven { attempt, error })
            }
            ExtraEvent::PreviousBoxKept { name, reason } => {
                Self::Workflow(WorkflowEvent::PreviousBoxKept { name, reason })
            }
            ExtraEvent::AgentPromptCheckFailed { error } => Self::AgentPromptCheckFailed { error },
            ExtraEvent::TranscriptSaved {
                trigger,
                path,
                digest,
            } => Self::TranscriptSaved {
                trigger,
                path,
                digest,
            },
            ExtraEvent::TranscriptFailed {
                trigger,
                error,
                unlinked,
            } => Self::TranscriptFailed {
                trigger,
                error,
                unlinked,
            },
            ExtraEvent::TranscriptSkipped { trigger, reason } => {
                Self::TranscriptSkipped { trigger, reason }
            }
            ExtraEvent::Restore(r) => Self::Restore(r),
            ExtraEvent::Steered(s) => Self::Steered(s),
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
