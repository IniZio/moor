//! A warmup whose caller was cut off goes on, and the next caller takes it over.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use std::path::Path;
use std::time::Duration;

use moor_client::link::lease::{Answer, Boot, Verb};
use moor_client::link::ops::Reply;
use moor_machine_agent::pool::{lease, Pool};

const SLUG: &str = "moor-1a2b3c4d-7";
const DONE: Reply = Reply::Leased(Answer::Done);

/// A crabbox whose warmup waits for `$HOME/go`, then lists its lease; it refuses a second warmup
/// and logs each verb when it ends.
const SCRIPT: &str = r#"#!/bin/sh
case "$1" in
  warmup)
    if ! mkdir "$HOME/claim" 2>/dev/null; then
      echo "requested external lease slug is already in use" >&2; exit 4
    fi
    while [ ! -e "$HOME/go" ]; do sleep 0.02; done
    [ -e "$HOME/warmup-fails" ] && { rmdir "$HOME/claim"; echo "no room" >&2; exit 2; }
    echo '[{"name":"x","slug":"moor-1a2b3c4d-7","state":"running"}]' > "$HOME/leases.json"
    ;;
  stop) rm -rf "$HOME/claim" "$HOME/leases.json" ;;
  list) cat "$HOME/leases.json" 2>/dev/null || echo '[]' ;;
esac
echo "$1" >> "$HOME/calls"
"#;

fn pool(home: &Path) -> Pool {
    let crabbox = home.join("crabbox");
    moor_testkit::write_executable(&crabbox, SCRIPT).unwrap();
    Pool {
        provider: "fake".to_owned(),
        max: None,
        reserve_mib: None,
        cache_gb: None,
        crabbox,
    }
}

fn boots(home: &Path) -> usize {
    calls(home).iter().filter(|c| *c == "warmup").count()
}

fn calls(home: &Path) -> Vec<String> {
    std::fs::read_to_string(home.join("calls"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

async fn ask(home: &Path, pool: &Pool, verb: Verb) -> Reply {
    lease(home, pool, verb, Some(SLUG), None, &Boot::default()).await
}

/// Starts a warmup and cuts its caller off once crabbox has claimed the slug.
async fn cut_warmup(home: &Path, pool: &Pool) {
    let (h, p) = (home.to_path_buf(), pool.clone());
    let caller = tokio::spawn(async move { ask(&h, &p, Verb::Warmup).await });
    while !home.join("claim").exists() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    caller.abort();
    let _ = caller.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_adopt_waits_for_a_warmup_whose_caller_was_cut_off() {
    let home = tempfile::tempdir().unwrap();
    let pool = pool(home.path());
    cut_warmup(home.path(), &pool).await;
    let go = home.path().join("go");
    let late = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        std::fs::write(go, "").unwrap();
    });
    assert_eq!(ask(home.path(), &pool, Verb::Adopt).await, DONE);
    late.await.unwrap();
    assert_eq!(boots(home.path()), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_adopt_after_the_warmup_ended_finds_its_vm_and_boots_nothing() {
    let home = tempfile::tempdir().unwrap();
    let pool = pool(home.path());
    std::fs::write(home.path().join("go"), "").unwrap();
    assert_eq!(ask(home.path(), &pool, Verb::Warmup).await, DONE);
    assert_eq!(ask(home.path(), &pool, Verb::Adopt).await, DONE);
    assert_eq!(boots(home.path()), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_adopt_with_nothing_to_take_over_boots_the_vm() {
    let home = tempfile::tempdir().unwrap();
    let pool = pool(home.path());
    std::fs::write(home.path().join("go"), "").unwrap();
    assert_eq!(ask(home.path(), &pool, Verb::Adopt).await, DONE);
    assert_eq!(boots(home.path()), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_adopt_of_a_warmup_that_fails_says_why() {
    let home = tempfile::tempdir().unwrap();
    let pool = pool(home.path());
    std::fs::write(home.path().join("warmup-fails"), "").unwrap();
    cut_warmup(home.path(), &pool).await;
    std::fs::write(home.path().join("go"), "").unwrap();
    let got = ask(home.path(), &pool, Verb::Adopt).await;
    assert!(
        matches!(&got, Reply::Refused { reason } if reason.contains("no room")),
        "{got:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_waits_for_the_warmup_so_it_leaves_no_vm_behind() {
    let home = tempfile::tempdir().unwrap();
    let pool = pool(home.path());
    cut_warmup(home.path(), &pool).await;
    let go = home.path().join("go");
    let late = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        std::fs::write(go, "").unwrap();
    });
    assert_eq!(ask(home.path(), &pool, Verb::Stop).await, DONE);
    late.await.unwrap();
    assert_eq!(calls(home.path()), ["warmup", "stop"]);
    assert!(!home.path().join("leases.json").exists());
}
