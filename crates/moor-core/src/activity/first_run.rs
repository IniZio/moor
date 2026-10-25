//! The stop at a first-run screen, for an agent that runs no hook while one is up.

use super::{act, Activity, AgentState, Source};

/// The stop at the first-run screen named `name`. It offers no options: moor's pre-seed should
/// have skipped the screen, so it is reported as a launch fault rather than answered.
pub(super) fn first_run_wait(name: &str) -> Activity {
    act(
        AgentState::NeedsInput {
            question: Some(format!("the agent stopped at its {name} screen")),
            options: Vec::new(),
        },
        Source::FirstRun,
    )
}
