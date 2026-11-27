//! `moor start` makes a Box with no agent, and `moor ask` starts a task in a place.

use moor_core::seam::forge::ForgeRepo;
use moor_core::Request;

use super::{req, run};

#[test]
fn start_takes_no_flag_only_an_agent_reads() {
    for agent_only in [
        &["--prompt", "g"][..],
        &["--issue", "I-1"],
        &["--model", "haiku"],
        &["--no-agent"],
        &["--resume"],
    ] {
        let args: Vec<&str> = ["start", "t1"].iter().chain(agent_only).copied().collect();
        assert!(run(&args).is_err(), "{agent_only:?}");
    }
}

#[test]
fn ask_starts_a_task_in_a_place() {
    assert!(matches!(
        req(&["ask", "--repo", "o/a", "Fix the typo"]),
        Request::Ask(a) if a.prompt == "Fix the typo" && a.repos == [ForgeRepo::new("o", "a")]
    ));
}
