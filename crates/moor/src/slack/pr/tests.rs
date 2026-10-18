use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use super::*;
use crate::adapters::clock::SystemClock;
use moor_core::seam::clock::Clock;
use moor_core::BoxName;

fn thread() -> SlackThread {
    SlackThread {
        channel: "C1".into(),
        thread_ts: "1.0".into(),
        task: BoxName::parse_allow_bare("fix-bug").unwrap(),
        owner: "U1".into(),
        status_ts: None,
    }
}

fn closed() -> HashMap<String, View> {
    let mut v = View::default();
    v.fold("closed", &serde_json::json!({}));
    HashMap::from([("fix-bug".to_owned(), v)])
}

#[test]
fn lookups_are_spaced_by_the_refresh_interval() {
    let t0 = SystemClock::real().instant();
    let calls = Arc::new(AtomicUsize::new(0));
    let n = Arc::clone(&calls);
    let pr_of = move |_: &str| {
        n.fetch_add(1, Ordering::SeqCst);
        Some("https://x/pull/1".to_owned())
    };
    let mut c = PrCache::default();
    let th = [thread()];
    let views = HashMap::new();
    c.refresh(&th, &views, &pr_of, t0);
    c.refresh(&th, &views, &pr_of, t0 + Duration::from_secs(59));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    c.refresh(&th, &views, &pr_of, t0 + REFRESH_EVERY);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(c.urls().get("fix-bug").unwrap(), "https://x/pull/1");
}

#[test]
fn closed_tasks_are_not_looked_up() {
    let t0 = SystemClock::real().instant();
    let calls = Arc::new(AtomicUsize::new(0));
    let n = Arc::clone(&calls);
    let pr_of = move |_: &str| {
        n.fetch_add(1, Ordering::SeqCst);
        None
    };
    let mut c = PrCache::default();
    c.refresh(&[thread()], &closed(), &pr_of, t0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn failed_lookup_keeps_the_known_url() {
    let t0 = SystemClock::real().instant();
    let found = Arc::new(AtomicBool::new(true));
    let f = Arc::clone(&found);
    let pr_of = move |_: &str| {
        f.load(Ordering::SeqCst)
            .then(|| "https://x/pull/1".to_owned())
    };
    let mut c = PrCache::default();
    let th = [thread()];
    c.refresh(&th, &HashMap::new(), &pr_of, t0);
    found.store(false, Ordering::SeqCst);
    c.refresh(&th, &HashMap::new(), &pr_of, t0 + REFRESH_EVERY);
    assert_eq!(c.urls().get("fix-bug").unwrap(), "https://x/pull/1");
}
