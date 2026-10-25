//! The wait at a first-run screen, for an agent that runs no hook while one is up.

use super::{act, menu_options, Activity, AgentState, Source};

/// The wait at the first-run screen named `name`, with the menu the pane `tail` shows.
pub(super) fn first_run_wait(name: &str, tail: &str) -> Activity {
    act(
        AgentState::NeedsInput {
            question: Some(format!("the agent waits at its {name} prompt")),
            options: menu_options(tail),
        },
        Source::FirstRun,
    )
}
