use std::sync::Arc;
use std::time::Duration;

use moor_core::name::BoxHandle;
use moor_core::seam::driver::{Driver, DriverError, SleepOutcome};
use moor_core::seam::proc::Tool;

use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::{Reply, ScriptedProc};

const PROVIDERS: &str = r#"[
 {"provider":"pauser","features":["pause-resume"]},
 {"provider":"sprites","features":["native-sleep"]},
 {"provider":"plain"},
 {"provider":"namespace-devbox","features":["resume-rotates-host-key"],"lifecycle":["pause-resume"]}
]"#;

fn fixture() -> (Arc<ScriptedProc>, Crabbox) {
    let p = Arc::new(ScriptedProc::new());
    p.on(
        Tool::Crabbox,
        &["providers"],
        Reply::Ok(PROVIDERS.to_owned()),
    );
    let c = Crabbox::fake(p.clone());
    (p, c)
}

fn verbs(p: &ScriptedProc) -> Vec<String> {
    p.calls()
        .into_iter()
        .filter_map(|c| c.args.into_iter().next())
        .collect()
}

fn fail(stderr: &str) -> Reply {
    Reply::Fail {
        code: 1,
        stdout: String::new(),
        stderr: stderr.to_owned(),
    }
}

#[test]
fn suspend_provider_pauses_by_provider_and_id() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["pause"], Reply::Ok(String::new()));
    let got = c.sleep(&BoxHandle::new("pauser/box"));
    assert_eq!(got, Ok(SleepOutcome::Slept));
    let call = p
        .calls()
        .into_iter()
        .find(|c| c.args.first().is_some_and(|a| a == "pause"))
        .unwrap();
    assert_eq!(call.args, ["pause", "--provider", "pauser", "--id", "box"]);
    assert_eq!(call.deadline.0, Duration::from_mins(2));
}

#[test]
fn native_provider_makes_no_pause_call() {
    let (p, c) = fixture();
    assert_eq!(
        c.sleep(&BoxHandle::new("sprites/box")),
        Ok(SleepOutcome::Native)
    );
    assert_eq!(verbs(&p), ["providers"]);
}

#[test]
fn provider_that_cannot_sleep_gives_a_reason_and_no_call() {
    let (p, c) = fixture();
    let got = c.sleep(&BoxHandle::new("plain/box"));
    assert!(matches!(&got, Ok(SleepOutcome::CannotSleep(r)) if r.contains("plain")));
    assert_eq!(verbs(&p), ["providers"]);
}

#[test]
fn provider_that_cannot_wake_stays_up() {
    let (p, c) = fixture();
    let got = c.sleep(&BoxHandle::new("namespace-devbox/box"));
    assert!(matches!(&got, Ok(SleepOutcome::CannotSleep(r)) if r.contains("wake")));
    assert_eq!(verbs(&p), ["providers"]);
}

#[test]
fn unknown_provider_stays_up_with_a_reason_and_no_pause() {
    let (p, c) = fixture();
    let got = c.sleep(&BoxHandle::new("nope/box"));
    assert!(matches!(&got, Ok(SleepOutcome::CannotSleep(r)) if r.contains("nope")));
    assert_eq!(verbs(&p), ["providers"]);
}

#[test]
fn handle_without_provider_cannot_sleep() {
    let (p, c) = fixture();
    let got = c.sleep(&BoxHandle::new("box"));
    assert!(matches!(got, Ok(SleepOutcome::CannotSleep(_))));
    assert!(p.calls().is_empty());
}

#[test]
fn failed_pause_is_an_error_without_raw_stderr() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["pause"], fail("SECRET detail"));
    let e = c.sleep(&BoxHandle::new("pauser/box")).unwrap_err();
    assert!(matches!(e, DriverError::Failed { .. }));
    assert!(!e.to_string().contains("SECRET"));
}

#[test]
fn pause_timeout_is_a_timeout() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["pause"], Reply::Timeout);
    assert!(matches!(
        c.sleep(&BoxHandle::new("pauser/box")),
        Err(DriverError::Timeout(_))
    ));
}

#[test]
fn wake_resumes_by_provider_and_id() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["resume"], Reply::Ok(String::new()));
    assert_eq!(c.wake(&BoxHandle::new("pauser/box")), Ok(()));
    let calls = p.calls();
    assert_eq!(verbs(&p), ["providers", "resume"]);
    assert_eq!(
        calls.last().unwrap().args,
        ["resume", "--provider", "pauser", "--id", "box"]
    );
}

#[test]
fn failed_resume_is_an_error_without_raw_stderr() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["resume"], fail("SECRET detail"));
    let e = c.wake(&BoxHandle::new("pauser/box")).unwrap_err();
    assert!(matches!(e, DriverError::Failed { .. }));
    assert!(!e.to_string().contains("SECRET"));
    assert!(e.to_string().contains("box wake"));
}

#[test]
fn wake_of_native_provider_makes_only_the_providers_call() {
    let (p, c) = fixture();
    assert_eq!(c.wake(&BoxHandle::new("sprites/box")), Ok(()));
    assert_eq!(verbs(&p), ["providers"]);
}

#[test]
fn wake_is_unsupported_without_sleep_or_provider() {
    let (p, c) = fixture();
    assert!(matches!(
        c.wake(&BoxHandle::new("plain/box")),
        Err(DriverError::Unsupported(_))
    ));
    assert!(matches!(
        c.wake(&BoxHandle::new("box")),
        Err(DriverError::Unsupported(_))
    ));
    assert!(matches!(
        c.wake(&BoxHandle::new("nope/box")),
        Err(DriverError::Unsupported(_))
    ));
    assert_eq!(verbs(&p), ["providers", "providers"]);
}
