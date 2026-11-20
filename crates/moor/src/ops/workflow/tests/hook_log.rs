//! The one line each delivery to the App's webhook leaves in the controller's log: refused with
//! its reason, ignored with its event and action, or accepted with what it led to.
#![allow(clippy::panic, reason = "a test names what it found when it fails")]

use std::io::Write;
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::hook::{body, delivery, hooked, Hooked, NEW, OLD, SECRET};
use crate::ops::common::stale::ControllerLog;

/// A rig whose controller log lands in the returned buffer.
fn logged() -> (Hooked, Arc<Mutex<Vec<u8>>>) {
    let mut r = hooked();
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink: Arc<Mutex<dyn Write + Send>> = buf.clone();
    r.rig.cx = r.rig.cx.sharing(ControllerLog::new(&sink));
    (r, buf)
}

/// The lines logged so far.
fn lines(buf: &Mutex<Vec<u8>>) -> Vec<String> {
    String::from_utf8(buf.lock().unwrap().clone())
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_bad_or_missing_signature_logs_one_refusal_without_the_signature_or_body() {
    let (r, buf) = logged();
    let b = body("opened", ("o/n", 65), OLD);
    let forged = delivery("pull_request", b.clone(), Some(b"not-the-secret"));
    let secret_sig = forged.signature.clone().unwrap();
    r.deliver(&forged).unwrap();
    r.deliver(&delivery("pull_request", b, None)).unwrap();
    let got = lines(&buf);
    assert_eq!(
        got,
        [
            "moor controller: github delivery d-1: refused, bad or missing signature (event pull_request)",
            "moor controller: github delivery d-1: refused, bad or missing signature (event pull_request)",
        ]
    );
    let all = got.join("\n");
    assert!(!all.contains(&secret_sig) && !all.contains(OLD), "{all}");
}

#[test]
fn a_controller_without_a_secret_logs_that_it_refused_for_that() {
    let mut r = super::rig();
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink: Arc<Mutex<dyn Write + Send>> = buf.clone();
    r.cx = r.cx.sharing(ControllerLog::new(&sink));
    let req = delivery("ping", "{}".to_owned(), Some(SECRET));
    r.deliver(&req).unwrap();
    assert_eq!(
        lines(&buf),
        ["moor controller: github delivery d-1: refused, no webhook secret is configured (event ping)"]
    );
}

#[test]
fn a_signed_body_that_does_not_parse_logs_a_refusal_without_the_body() {
    let (r, buf) = logged();
    let req = delivery(
        "pull_request",
        "{\"action\":\"opened\",\"number\":\"hush\"}".to_owned(),
        Some(SECRET),
    );
    assert!(r.deliver(&req).is_err());
    assert_eq!(
        lines(&buf),
        ["moor controller: github delivery d-1: refused, the pull_request body does not parse"]
    );
}

#[test]
fn an_event_or_action_moor_does_not_handle_logs_one_ignored_line() {
    let (r, buf) = logged();
    r.put_pr("evidence", true, true);
    for (event, b) in [
        (
            "ping",
            json!({"zen": "Keep it logically awesome."}).to_string(),
        ),
        ("issue_comment", json!({"action": "created"}).to_string()),
        ("pull_request", body("labeled", ("o/n", 65), OLD)),
    ] {
        r.deliver(&delivery(event, b, Some(SECRET))).unwrap();
    }
    assert_eq!(
        lines(&buf),
        [
            "moor controller: github delivery d-1: ignored ping",
            "moor controller: github delivery d-1: ignored issue_comment/created",
            "moor controller: github delivery d-1: ignored pull_request/labeled",
        ]
    );
}

#[test]
fn an_accepted_delivery_logs_one_line_with_what_it_led_to() {
    let (r, buf) = logged();
    r.signed("opened", 65, OLD);
    r.put_pr("evidence", true, true);
    r.signed("opened", 66, OLD);
    r.signed("synchronize", 66, NEW);
    r.signed("opened", 66, NEW);
    let at = |sha: &str| sha.chars().take(7).collect::<String>();
    let pre = "moor controller: github delivery d-1: accepted pull_request";
    let (old, new) = (at(OLD), at(NEW));
    assert_eq!(
        lines(&buf),
        [
            format!("{pre}/opened o/n#65 at {old}: no workflow wants it"),
            format!("{pre}/opened o/n#66 at {old}: workflow evidence on o/n#66 at {old}: started"),
            format!("{pre}/synchronize o/n#66 at {new}: workflow evidence on o/n#66 at {new}: started"),
            format!("{pre}/opened o/n#66 at {new}: workflow evidence on o/n#66 at {new}: refused: a run of evidence for this pull request at this head is already recorded"),
        ]
    );
}
