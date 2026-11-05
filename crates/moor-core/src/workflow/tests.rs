//! Tests for workflow parsing.

use std::time::Duration;

use super::{TimeoutMinutes, WorkflowDef, WorkflowError, WorkflowName, WorkflowSchedule};
use crate::name::ForgeRepo;
use crate::seam::clock::Ts;

fn repo(s: &str) -> ForgeRepo {
    ForgeRepo::parse(s).unwrap()
}

fn schedule() -> WorkflowSchedule {
    WorkflowSchedule::parse("0 9 * * *", "Europe/London").unwrap()
}

#[test]
fn names_are_lowercase_and_bounded() {
    for ok in ["daily-fix", "a", "sec.fix_2", &"a".repeat(64)] {
        assert!(WorkflowName::parse(ok).is_ok(), "{ok}");
    }
    for bad in ["", "-x", "Daily", "a b", "a/b", &"a".repeat(65)] {
        assert_eq!(
            WorkflowName::parse(bad),
            Err(WorkflowError::Name(bad.to_owned())),
            "{bad}"
        );
    }
}

#[test]
fn schedule_needs_five_fields_and_an_iana_zone() {
    assert!(WorkflowSchedule::parse("0 */2 * * 1-5", "UTC").is_ok());
    for bad in ["", "* * * *", "0 0 9 * * *", "61 * * * *", "daily"] {
        assert!(
            matches!(
                WorkflowSchedule::parse(bad, "UTC"),
                Err(WorkflowError::Cron { .. })
            ),
            "{bad}"
        );
    }
    assert_eq!(
        WorkflowSchedule::parse("0 9 * * *", "Mars/Olympus"),
        Err(WorkflowError::Zone("Mars/Olympus".to_owned()))
    );
}

#[test]
fn schedule_finds_firing_times_in_its_zone() {
    let london = schedule();
    // 2026-07-01 12:00 UTC is 13:00 BST, so 09:00 BST that day is 08:00 UTC.
    let noon = Ts::from_millis(1_782_907_200_000);
    let nine = Ts::from_millis(1_782_892_800_000);
    assert_eq!(london.latest_at_or_before(noon), Some(nine));
    assert_eq!(london.latest_at_or_before(nine), Some(nine));
    assert_eq!(
        london.first_after(nine),
        Some(Ts::from_millis(nine.as_millis() + 86_400_000))
    );
}

#[test]
fn a_definition_needs_repos_and_a_prompt() {
    let name = || WorkflowName::parse("fix").unwrap();
    assert!(WorkflowDef::new(
        name(),
        schedule(),
        vec![repo("a/b")],
        "go",
        (true, TimeoutMinutes::DEFAULT)
    )
    .is_ok());
    assert_eq!(
        WorkflowDef::new(
            name(),
            schedule(),
            vec![],
            "go",
            (true, TimeoutMinutes::DEFAULT)
        ),
        Err(WorkflowError::NoRepos)
    );
    assert_eq!(
        WorkflowDef::new(
            name(),
            schedule(),
            vec![repo("a/b"), repo("a/b")],
            "go",
            (true, TimeoutMinutes::DEFAULT)
        ),
        Err(WorkflowError::DuplicateRepo("a/b".to_owned()))
    );
    assert_eq!(
        WorkflowDef::new(
            name(),
            schedule(),
            vec![repo("a/b")],
            "  ",
            (true, TimeoutMinutes::DEFAULT)
        ),
        Err(WorkflowError::NoPrompt)
    );
}

#[test]
fn a_timeout_is_one_to_720_minutes_and_defaults_to_three_hours() {
    assert_eq!(TimeoutMinutes::default().minutes(), 180);
    assert_eq!(TimeoutMinutes::new(1).unwrap().minutes(), 1);
    assert_eq!(TimeoutMinutes::new(720).unwrap().minutes(), 720);
    for bad in [0, 721] {
        assert_eq!(TimeoutMinutes::new(bad), Err(WorkflowError::Timeout(bad)));
    }
}

fn too_frequent(expr: &str, zone: &str) -> bool {
    matches!(
        WorkflowSchedule::parse(expr, zone),
        Err(WorkflowError::TooFrequent { .. })
    )
}

#[test]
fn a_schedule_that_can_fire_within_the_floor_is_rejected() {
    for expr in ["* * * * *", "*/15 * * * *", "0,30 * * * *"] {
        assert!(too_frequent(expr, "UTC"), "{expr}");
    }
    for ok in [
        "0 * * * *",
        "0 */2 * * *",
        "0 9 * * *",
        "0 9 * * 1",
        "0 0 1 * *",
    ] {
        assert!(WorkflowSchedule::parse(ok, "UTC").is_ok(), "{ok}");
    }
}

#[test]
fn the_floor_sees_gaps_that_only_some_firings_have() {
    // Fires at 00:00 and 00:30 only on the 1st: nothing else in the month is close.
    assert!(too_frequent("0,30 0 1 * *", "UTC"));
    // Mondays and the 2nd are consecutive days some months, never closer than a day.
    assert!(!too_frequent("0 9 2 * 1", "UTC"));
}

#[test]
fn the_floor_is_a_parameter() {
    let every_ten = "*/10 * * * *";
    assert!(
        WorkflowSchedule::parse_with_min_interval(every_ten, "UTC", Duration::from_secs(600))
            .is_ok()
    );
    assert!(
        WorkflowSchedule::parse_with_min_interval(every_ten, "UTC", Duration::from_secs(601))
            .is_err()
    );
    assert!(WorkflowSchedule::parse_with_min_interval("* * * * *", "UTC", Duration::ZERO).is_ok());
}

#[test]
fn a_daily_time_on_a_clock_change_day_is_one_firing() {
    // croner 4.0.2 also fired 03:00 on the spring-forward day for 01:30, which the floor caught.
    assert!(WorkflowSchedule::parse("30 1 * * *", "America/New_York").is_ok());
    assert!(WorkflowSchedule::parse("30 2 * * *", "America/New_York").is_ok());
    assert!(WorkflowSchedule::parse("30 1 * * *", "Europe/London").is_ok());
}

fn named(id: i64, workflow: &str, r: &str, due: i64) -> String {
    super::box_name(
        super::WorkflowId::new(id),
        workflow,
        &repo(r),
        Ts::from_millis(due),
    )
    .unwrap()
    .as_str()
    .to_owned()
}

#[test]
fn a_firing_names_its_box_by_repo_workflow_and_due_time() {
    // 2026-07-01 09:00 UTC.
    assert_eq!(
        named(7, "daily-fix", "acme/web", 1_782_896_400_000),
        "web/w7-daily-fix-202607010900"
    );
}

#[test]
fn box_names_stay_valid_and_apart_for_awkward_input() {
    let long = "a".repeat(64);
    let name = named(12, &long, "Acme/My.Repo_Name", 1_782_896_400_000);
    assert!(crate::BoxName::parse(&name).is_ok(), "{name}");
    assert!(name.starts_with("my-repo-name/w12-aaaa"), "{name}");
    assert_ne!(
        named(1, "x", "a/b", 1_782_896_400_000),
        named(2, "x", "a/b", 1_782_896_400_000)
    );
    assert_ne!(
        named(1, "x", "a/b", 1_782_896_400_000),
        named(1, "x", "a/b", 1_782_982_800_000)
    );
    assert!(crate::BoxName::parse(&named(1, "__", "a/---", 0)).is_ok());
}
