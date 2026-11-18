//! Tests for the derived Box state, its line, the PR link and the timeline.

use super::*;
use crate::domain::event::EVENT_KINDS;
use crate::domain::hands::HandsState;
use crate::view::attention::AttentionView;
use crate::view::{AgentView, BoxSleepView, DriverView, LaunchFailure};
use serde_json::json;

fn row(lifecycle: BoxLifecycle, session: SessionState) -> BoxView {
    BoxView {
        id: 1,
        name: "r/t".into(),
        repo: Some(crate::seam::forge::ForgeRepo::new("Acme", "web")),
        other_repos: Vec::new(),
        issue: None,
        prompt: None,
        lifecycle,
        session_state: session,
        unfinished_turn: false,
        state_reason: None,
        base: None,
        branch: None,
        driver: Some(DriverView {
            state: HandsState::Ready,
            provider: "p".into(),
            handle: None,
            workdir: None,
            egress: None,
            cpus: None,
            mem_mib: None,
            asleep_since: None,
            sleep: BoxSleepView {
                after: None,
                reason: None,
            },
        }),
        agent: None,
        run: None,
        updated_at: "2026-11-18T10:00:00Z".into(),
        attention: Vec::new(),
        held_machine: None,
        said: None,
        progress: None,
    }
}

fn agent(state: &str) -> AgentView {
    AgentView {
        launched: true,
        pane: "live".into(),
        state: state.into(),
        source: "status".into(),
        exit_code: None,
        question: None,
        options: Vec::new(),
        waiting_on: Vec::new(),
        reason: None,
    }
}

fn ev(id: i64, kind: &str, ts: &str, payload: Value) -> EventView {
    EventView {
        id,
        kind: kind.into(),
        payload,
        ts: ts.into(),
    }
}

/// 2026-11-18T10:00:00Z in unix milliseconds.
fn ten() -> i64 {
    unix("2026-11-18T10:00:00Z").unwrap() * 1000
}

fn state(b: &BoxView) -> Progress {
    derive(b, &[], ten()).state
}

#[test]
fn lifecycle_decides_the_states_before_the_agent() {
    use BoxLifecycle as L;
    let cases = [
        (L::Pending, Progress::Queued),
        (L::Checking, Progress::SettingUp),
        (L::SettingUp, Progress::SettingUp),
        (L::SetupFailed, Progress::Failed),
        (L::Rejected, Progress::Failed),
        (L::Asleep, Progress::Asleep),
        (L::Closed, Progress::Closed),
    ];
    for (lifecycle, want) in cases {
        let mut b = row(lifecycle, SessionState::Working);
        b.agent = Some(agent("needs_input"));
        assert_eq!(state(&b), want, "{lifecycle:?}");
    }
}

#[test]
fn a_ready_box_follows_its_session_and_agent() {
    let mut b = row(BoxLifecycle::Ready, SessionState::Working);
    assert_eq!(state(&b), Progress::Working);
    b.session_state = SessionState::Starting;
    assert_eq!(state(&b), Progress::Working);
    b.session_state = SessionState::Idle;
    assert_eq!(state(&b), Progress::Done);
    b.session_state = SessionState::Ended;
    assert_eq!(state(&b), Progress::Done);
}

#[test]
fn a_question_needs_you_and_its_text_is_the_line() {
    let mut b = row(BoxLifecycle::Ready, SessionState::Working);
    let mut a = agent("needs_input");
    a.question = Some("Allow npm run e2e?".into());
    b.agent = Some(a);
    let t = derive(&b, &[], ten());
    assert_eq!(
        (t.state, t.line.as_str()),
        (Progress::NeedsYou, "Allow npm run e2e?")
    );
}

#[test]
fn a_question_missing_from_the_row_comes_from_the_event() {
    let mut b = row(BoxLifecycle::Ready, SessionState::Working);
    b.agent = Some(agent("needs_input"));
    let e = [ev(
        1,
        "needs_input",
        "2026-11-18T09:59:00Z",
        json!({"question": "Deploy?"}),
    )];
    assert_eq!(derive(&b, &e, ten()).line, "Deploy?");
}

#[test]
fn an_open_credential_limit_needs_you() {
    let mut b = row(BoxLifecycle::Ready, SessionState::Working);
    b.attention = vec![AttentionView {
        kind: "agent_rate_limited".into(),
        rule: "claude".into(),
        status: 429,
        resets_at: None,
        message: "Claude account rate limited (429)".into(),
    }];
    let t = derive(&b, &[], ten());
    assert_eq!(t.state, Progress::NeedsYou);
    assert_eq!(t.line, "Claude account rate limited (429)");
}

#[test]
fn an_agent_waiting_on_background_work_is_still_working() {
    let mut b = row(BoxLifecycle::Ready, SessionState::Idle);
    b.agent = Some(agent("waiting"));
    assert_eq!(state(&b), Progress::Working);
}

#[test]
fn a_crashed_agent_and_a_failed_launch_fail_but_a_clean_exit_is_done() {
    let mut b = row(BoxLifecycle::Ready, SessionState::Ended);
    let mut a = agent("exited");
    a.exit_code = Some(1);
    b.agent = Some(a.clone());
    assert_eq!(state(&b), Progress::Failed);
    a.exit_code = Some(0);
    b.agent = Some(a);
    assert_eq!(state(&b), Progress::Done);
    let mut a = agent("failed");
    a.reason = Some(LaunchFailure {
        step: "launch".into(),
        message: "no claude".into(),
        tail: None,
        at: "t".into(),
    });
    b.agent = Some(a);
    let t = derive(&b, &[], ten());
    assert_eq!(t.state, Progress::Failed);
    assert!(t.line.contains("no claude"), "{}", t.line);
}

#[test]
fn a_failed_setup_says_why_from_the_events() {
    let b = row(BoxLifecycle::SetupFailed, SessionState::Starting);
    let e = [ev(
        1,
        "setup_failed",
        "2026-11-18T09:59:00Z",
        json!({"line": "bun install exited 1"}),
    )];
    assert!(derive(&b, &e, ten()).line.contains("bun install exited 1"));
}

#[test]
fn a_setting_up_box_reads_its_latest_step() {
    let b = row(BoxLifecycle::SettingUp, SessionState::Starting);
    let e = [ev(1, "cloned", "2026-11-18T09:59:00Z", json!({"ms": 4000}))];
    assert_eq!(derive(&b, &e, ten()).line, "setting up · cloned in 4s");
}

#[test]
fn only_a_working_box_goes_stale_after_thirty_minutes_without_an_event() {
    let mut b = row(BoxLifecycle::Ready, SessionState::Working);
    let e = [ev(
        1,
        "agent_doing",
        "2026-11-18T09:30:00Z",
        json!({"text": "running cargo test"}),
    )];
    let fresh = derive(&b, &e, ten() - 1);
    assert!(!fresh.stale, "30 minutes exactly is not stale");
    let old = derive(&b, &e, ten() + 1000);
    assert!(old.stale);
    assert_eq!(old.line, "no event for 30m 01s · last: running cargo test");
    b.session_state = SessionState::Idle;
    assert!(
        !derive(&b, &e, ten() + 3_600_000).stale,
        "a done Box is quiet, not stale"
    );
}

#[test]
fn a_working_line_is_the_latest_tool_call_then_the_latest_message() {
    let b = row(BoxLifecycle::Ready, SessionState::Working);
    let said = ev(
        1,
        "agent_said",
        "2026-11-18T09:58:00Z",
        json!({"text": "Looking.\nMore."}),
    );
    assert_eq!(
        derive(&b, std::slice::from_ref(&said), ten()).line,
        "Looking."
    );
    let doing = ev(
        2,
        "agent_doing",
        "2026-11-18T09:59:00Z",
        json!({"text": "editing src/theme.ts"}),
    );
    assert_eq!(
        derive(&b, &[said, doing], ten()).line,
        "editing src/theme.ts"
    );
}

#[test]
fn the_pr_link_comes_only_from_the_boxes_own_repos() {
    let b = row(BoxLifecycle::Ready, SessionState::Idle);
    let said = |text: &str| {
        ev(
            1,
            "agent_said",
            "2026-11-18T09:58:00Z",
            json!({"text": text}),
        )
    };
    let own = said("Opened https://github.com/acme/web/pull/212 for review.");
    let t = derive(&b, &[own], ten());
    assert_eq!(
        t.pr,
        Some(PrLink {
            repo: "Acme/web".into(),
            number: 212
        })
    );
    assert_eq!(t.line, "PR #212");
    let other =
        said("See https://github.com/evil/web/pull/9 and https://github.com/acme/web/issues/3");
    assert_eq!(derive(&b, &[other], ten()).pr, None);
    let two = said(
        "Closed https://github.com/acme/web/pull/1, opened https://github.com/acme/web/pull/2).",
    );
    assert_eq!(derive(&b, &[two], ten()).pr.map(|p| p.number), Some(2));
}

#[test]
fn every_event_kind_is_classed_so_none_folds_by_accident() {
    let missing: Vec<&str> = EVENT_KINDS
        .iter()
        .copied()
        .filter(|k| Class::of(k).is_none())
        .collect();
    assert!(
        missing.is_empty(),
        "class these kinds in progress/timeline.rs: {missing:?}"
    );
}

#[test]
fn the_timeline_hides_noise_keeps_errors_and_shows_an_unclassed_kind() {
    let events = [
        ev(1, "proxy_started", "t", json!({})),
        ev(2, "agent_doing", "t", json!({"text": "Read 2 files"})),
        ev(
            3,
            "setup_failed",
            "t",
            json!({"line": "bun install exited 1"}),
        ),
        ev(4, "brand_new_kind", "t", json!({})),
        ev(
            5,
            "artifact_published",
            "t",
            json!({"name": "after.png", "version": 2}),
        ),
        ev(
            6,
            "turn_done",
            "t",
            json!({"steer": 1, "outcome": "needs_input"}),
        ),
    ];
    let t = timeline(&events);
    let seen: Vec<(i64, Class)> = t.iter().map(|e| (e.id, e.class)).collect();
    assert_eq!(
        seen,
        [
            (2, Class::Step),
            (3, Class::Error),
            (4, Class::Error),
            (5, Class::Evidence),
            (6, Class::Turn)
        ]
    );
    assert!(
        t.get(1)
            .is_some_and(|e| e.text.contains("bun install exited 1")),
        "{t:?}"
    );
    assert_eq!(
        t.get(3).and_then(|e| e.artifact.clone()),
        Some(("after.png".to_owned(), 2))
    );
    assert_eq!(
        t.get(4).map(|e| e.text.as_str()),
        Some("Turn ended (needs input)")
    );
}

#[test]
fn title_is_the_name_without_a_prompt() {
    assert_eq!(title_of(None, "r/t"), "r/t");
    assert_eq!(title_of(Some("  \n \n"), "r/t"), "r/t");
    assert_eq!(title_of(Some("## \n- "), "r/t"), "r/t");
}

#[test]
fn title_is_the_first_nonblank_line() {
    let prompt = "\n  Fix the typo on the login page\nthen open a PR";
    assert_eq!(
        title_of(Some(prompt), "n"),
        "Fix the typo on the login page"
    );
}

#[test]
fn title_stops_at_the_first_sentence() {
    let prompt = "Fix the login typo. Then open a PR with a screenshot.";
    assert_eq!(title_of(Some(prompt), "n"), "Fix the login typo");
    // a dot inside a file name or a version does not end the sentence
    let prompt = "Rename login.tsx to signin.tsx and bump to v1.2 everywhere";
    assert_eq!(title_of(Some(prompt), "n"), prompt);
}

#[test]
fn title_keeps_a_short_lead_going() {
    assert_eq!(
        title_of(Some("Hi. Please fix the build"), "n"),
        "Hi. Please fix the build"
    );
}

#[test]
fn title_drops_list_and_heading_marks() {
    assert_eq!(title_of(Some("# Plan\n- add dark mode"), "n"), "Plan");
    assert_eq!(title_of(Some("- add dark mode"), "n"), "add dark mode");
}

#[test]
fn title_is_cut_at_a_word_with_an_ellipsis() {
    let prompt = "Make the settings page keep its scroll position when the sidebar collapses and expands again";
    let title = title_of(Some(prompt), "n");
    assert!(title.ends_with('…'), "{title}");
    assert!(title.chars().count() <= TITLE_MAX, "{title}");
    assert!(prompt.starts_with(title.trim_end_matches('…')), "{title}");
    assert!(!title.trim_end_matches('…').ends_with("sid"), "{title}");
}

#[test]
fn title_counts_characters_not_bytes() {
    let prompt = "ü".repeat(TITLE_MAX);
    assert_eq!(title_of(Some(&prompt), "n"), prompt);
    let prompt = "ü".repeat(TITLE_MAX + 5);
    let title = title_of(Some(&prompt), "n");
    assert_eq!(title.chars().count(), TITLE_MAX);
    assert!(title.ends_with('…'));
}

#[test]
fn title_of_a_short_prompt_has_no_ellipsis() {
    assert_eq!(title_of(Some("Fix login typo"), "n"), "Fix login typo");
}

#[test]
fn derive_carries_the_title() {
    let mut b = row(BoxLifecycle::Ready, SessionState::Working);
    b.prompt = Some("Fix the login typo. Then more.".into());
    assert_eq!(derive(&b, &[], 0).title, "Fix the login typo");
    b.prompt = None;
    assert_eq!(derive(&b, &[], 0).title, "r/t");
}
