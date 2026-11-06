use serde_json::{json, Value};

use super::{failure, latest, Happened, Progress};

const T0: i64 = 1_793_000_000;

fn event(kind: &str, payload: Value, secs: i64) -> Happened {
    let at = time_at(T0 + secs);
    Happened {
        kind: kind.to_owned(),
        payload,
        ts: at,
    }
}

/// `unix` as the controller writes it.
fn time_at(unix: i64) -> String {
    let (days, rem) = (unix.div_euclid(86_400), unix.rem_euclid(86_400));
    assert_eq!(days, 20_752, "the fixture stays on 2026-10-26");
    format!(
        "2026-10-26T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[test]
fn each_setup_step_reads_as_words_with_its_time_since_the_start() {
    let events = [
        event("started", json!({}), 0),
        event("pending", json!({"max_boxes": 2}), 0),
        event("box_requested", json!({}), 3),
        event("box_ready", json!({}), 20),
        event("setting_up", json!({"from": "pending"}), 21),
        event("cloned", json!({"ms": 12_400}), 33),
        event("setup_finished", json!({"ms": 40_000, "ok": true}), 75),
    ];
    let p = Progress::of(&events, T0 + 80);
    assert_eq!(p.elapsed, Some(80));
    assert_eq!(
        p.steps,
        [
            "    +0s  waiting for a machine slot",
            "    +3s  machine requested",
            "   +20s  machine ready",
            "   +21s  setting up",
            "   +33s  cloned in 12s",
            "+1m 15s  setup finished in 40s",
        ]
    );
}

#[test]
fn failures_say_what_failed() {
    let events = [
        event("started", json!({}), 0),
        event(
            "credential_failed",
            json!({"which": "github", "error": "token expired"}),
            1,
        ),
        event("box_failed", json!({"error": "no capacity"}), 2),
        event(
            "setup_failed",
            json!({"line": "npm ERR! missing script"}),
            3,
        ),
    ];
    let steps = Progress::of(&events, T0 + 3).steps;
    assert_eq!(
        steps,
        [
            "    +1s  credential failed: github: token expired",
            "    +2s  machine failed: no capacity",
            "    +3s  setup failed: npm ERR! missing script",
        ]
    );
}

#[test]
fn only_the_latest_steps_are_kept() {
    let events: Vec<_> = (0..20)
        .map(|i| event("box_requested", json!({}), i))
        .collect();
    let steps = Progress::of(&events, T0 + 20).steps;
    assert_eq!(steps.len(), 8);
    assert!(steps.last().unwrap().starts_with("   +19s"));
}

#[test]
fn a_box_with_no_events_has_no_elapsed_time() {
    assert_eq!(Progress::of(&[], T0), Progress::default());
}

#[test]
fn a_setup_failure_explains_a_down_box_until_it_is_closed() {
    let failed = [
        event("started", json!({}), 0),
        event("setup_failed", json!({"line": "exit 1"}), 5),
        event("idled", json!({}), 6),
    ];
    assert_eq!(failure(&failed).as_deref(), Some("setup failed: exit 1"));
    let mut closed = failed.to_vec();
    closed.push(event("closed", json!({"from": "setup_failed"}), 9));
    assert_eq!(failure(&closed), None);
    assert_eq!(
        failure(&[event("rejected", json!({"reason": "no repo"}), 0)]).as_deref(),
        Some("rejected: no repo")
    );
}

#[test]
fn post_clone_steps_name_what_runs_and_how_long_it_took() {
    let events = [
        event("started", json!({}), 0),
        event("cloned", json!({"ms": 10_000}), 10),
        event("step_started", json!({"step": "dotfiles"}), 11),
        event(
            "step_finished",
            json!({"step": "dotfiles", "ms": 192_000, "ok": true}),
            203,
        ),
        event("step_started", json!({"step": "agent_install"}), 203),
    ];
    let p = Progress::of(&events, T0 + 210);
    assert_eq!(
        p.steps,
        [
            "   +10s  cloned in 10s",
            "   +11s  installing dotfiles",
            "+3m 23s  dotfiles in 3m 12s",
            "+3m 23s  installing the agent",
        ]
    );
    assert_eq!(latest(&events).as_deref(), Some("installing the agent"));
}

#[test]
fn a_failed_post_clone_step_says_how_long_it_ran() {
    let events = [
        event("started", json!({}), 0),
        event(
            "step_finished",
            json!({"step": "agent_install", "ms": 41_000, "ok": false}),
            41,
        ),
    ];
    assert_eq!(
        latest(&events).as_deref(),
        Some("agent install failed after 41s")
    );
}

#[test]
fn each_setup_line_shows_when_it_starts_and_when_it_ends() {
    let events = [
        event("started", json!({}), 0),
        event(
            "setup_line_started",
            json!({"index": 3, "total": 6, "line": "rustup toolchain install"}),
            5,
        ),
        event(
            "setup_line_finished",
            json!({"index": 3, "total": 6, "ms": 12_000, "ok": true}),
            17,
        ),
        event(
            "setup_line_finished",
            json!({"index": 4, "total": 6, "ms": 3_000, "ok": false}),
            20,
        ),
    ];
    let p = Progress::of(&events, T0 + 21);
    assert_eq!(
        p.steps,
        [
            "    +5s  setup 3/6: rustup toolchain install",
            "   +17s  setup 3/6 done in 12s",
            "   +20s  setup 4/6 failed after 3s",
        ]
    );
}

#[test]
fn the_whole_start_steps_read_as_plain_words() {
    let step = |name: &str, ms: u64, ok: bool| {
        latest(&[event(
            "step_finished",
            json!({"step": name, "ms": ms, "ok": ok}),
            0,
        )])
        .unwrap()
    };
    assert_eq!(step("acquire", 58_000, true), "machine acquired in 58s");
    assert_eq!(step("proxy", 2_000, true), "proxy up in 2s");
    assert_eq!(
        step("clone_others", 9_000, true),
        "other repos cloned in 9s"
    );
    assert_eq!(step("launch", 4_000, true), "agent launched in 4s");
    assert_eq!(step("plugins", 21_000, true), "plugins installed in 21s");
    assert_eq!(step("proxy", 30_000, false), "proxy failed after 30s");
    let started = event("step_started", json!({"step": "acquire"}), 0);
    assert_eq!(latest(&[started]).unwrap(), "acquiring the machine");
}

#[test]
fn a_checkpoint_says_how_long_it_paused_the_box_or_why_it_failed() {
    let ok = event(
        "checkpoint_taken",
        json!({"layer": 2, "duration_ms": 31_000, "error": null}),
        0,
    );
    assert_eq!(latest(&[ok]).unwrap(), "took setup checkpoint 2 in 31s");
    let bad = event(
        "checkpoint_taken",
        json!({"layer": 0, "duration_ms": 4_000, "error": "disk full"}),
        0,
    );
    assert_eq!(
        latest(&[bad]).unwrap(),
        "setup checkpoint 0 failed after 4s: disk full"
    );
}

#[test]
fn an_emptied_cache_reads_as_a_step_and_is_not_a_failure() {
    let events = [
        event("started", json!({}), 0),
        event(
            "cache_emptied",
            json!({"volume": "docker", "reason": "blob abc does not match its hash"}),
            1,
        ),
    ];
    let p = Progress::of(&events, T0 + 2);
    assert_eq!(
        p.steps.last().unwrap(),
        "    +1s  emptied cache volume docker: blob abc does not match its hash"
    );
    assert_eq!(failure(&events), None);
}

#[test]
fn a_cold_cache_reads_as_a_step_and_is_not_a_failure() {
    let events = [
        event("started", json!({}), 0),
        event(
            "cache_cold",
            json!({"reason": "switched_off", "detail": "cache volumes are switched off for p"}),
            1,
        ),
    ];
    let p = Progress::of(&events, T0 + 2);
    assert_eq!(
        p.steps.last().unwrap(),
        "    +1s  starting without cache volumes: cache volumes are switched off for p"
    );
    assert_eq!(failure(&events), None);
    assert_eq!(
        super::cold_cache(&events),
        Some((
            "switched_off".to_owned(),
            "cache volumes are switched off for p".to_owned()
        ))
    );
}
