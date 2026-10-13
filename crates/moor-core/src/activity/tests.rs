//! Tests for the activity decider.

use super::{
    decide, Activity, AgentState, HookCache, HookKind, HookRead, HookRecord, PaneObs, PaneRead,
    PaneStatus, Probes, Source,
};
use crate::seam::clock::Ts;

mod hooks;
mod panes;
mod quiet;

const NOW: i64 = 1_000_000;
/// A session age well past the startup grace.
const OLD: i64 = 3600;
const TASKS: &str = "2 background tasks running";

const fn ts(secs_ago: i64) -> Ts {
    Ts::from_millis(NOW - secs_ago * 1000)
}

fn rec(kind: HookKind) -> HookRecord {
    HookRecord {
        kind,
        session_id: None,
        at: ts(1),
    }
}

fn blocked(text: &str) -> HookKind {
    HookKind::Blocked {
        waiting_for: text.to_owned(),
    }
}

fn waiting(text: &str) -> AgentState {
    AgentState::Blocked {
        waiting_for: text.to_owned(),
    }
}

fn fresh(kind: HookKind) -> HookRead {
    HookRead::Fresh(rec(kind))
}

fn obs(status: PaneStatus, tail: &str) -> PaneRead {
    PaneRead::Fresh(PaneObs {
        status,
        tail: tail.to_owned(),
        exit_marker: None,
    })
}

fn exited(code: i32) -> PaneRead {
    PaneRead::Fresh(PaneObs {
        status: PaneStatus::Unknown,
        tail: format!("moor: agent exited {code}"),
        exit_marker: Some(code),
    })
}

fn cached(kind: HookKind, secs_ago: i64) -> HookCache {
    HookCache {
        record: rec(kind),
        at: ts(secs_ago),
    }
}

/// One decide scenario.
struct Case {
    name: &'static str,
    probes: Probes,
    cache: Option<HookCache>,
    started_ago: i64,
}

impl Case {
    const fn new(name: &'static str, hook: HookRead, pane: PaneRead) -> Self {
        Self {
            name,
            probes: Probes { hook, pane },
            cache: None,
            started_ago: OLD,
        }
    }

    fn cache(mut self, kind: HookKind, secs_ago: i64) -> Self {
        self.cache = Some(cached(kind, secs_ago));
        self
    }

    const fn started(mut self, secs_ago: i64) -> Self {
        self.started_ago = secs_ago;
        self
    }

    fn got(&self) -> Activity {
        decide(
            &self.probes,
            self.cache.as_ref(),
            ts(0),
            ts(self.started_ago),
        )
    }

    /// Asserts the decided state and source.
    fn expect(self, state: AgentState, source: Source) {
        assert_eq!(self.got(), Activity { state, source }, "{}", self.name);
    }
}

#[test]
fn busy_only_for_starting_working_unknown() {
    let cases = [
        (AgentState::Starting, true),
        (AgentState::Working, true),
        (AgentState::Unknown, true),
        (AgentState::Idle, false),
        (waiting("x"), false),
        (AgentState::Exited { code: 1 }, false),
    ];
    for (state, want) in cases {
        assert_eq!(state.busy(), want, "{state:?}");
    }
}
