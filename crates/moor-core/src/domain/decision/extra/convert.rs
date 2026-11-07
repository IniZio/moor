//! Conversion of a decision's events to stored events.

use super::ExtraEvent;
use crate::domain::event::{Event, NodeDiskEvent, ProfileEvent, StallEvent, TimingEvent};

impl From<ExtraEvent> for Event {
    #[allow(clippy::too_many_lines, reason = "one arm per event variant")]
    fn from(e: ExtraEvent) -> Self {
        match e {
            ExtraEvent::StaysUp { reason } => Self::StaysUp { reason },
            ExtraEvent::DiskLost => Self::DiskLost,
            ExtraEvent::CachesNotSaved => Self::CachesNotSaved,
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
