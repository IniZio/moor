//! A pool host's sweep of leases whose Box the controller no longer has.

use super::*;

const OPEN: &str = "moor-1a2b3c4d-7";
const STRAY: &str = "moor-1a2b3c4d-8";

/// A pool host on `home` with `leases` listed, its dirs made, and the given grace and cache.
async fn host(w: &World, home: &Path, grace: Duration, cache_gb: Option<u32>) -> tempfile::TempDir {
    let pool = moor_machine_agent::Pool {
        provider: "fake".to_owned(),
        max: None,
        reserve_mib: None,
        cache_gb,
        crabbox: fake_crabbox::install(home),
    };
    let leases = json!([{"slug": OPEN, "state": "running"}, {"slug": STRAY, "state": "paused"}]);
    std::fs::write(home.join("leases.json"), leases.to_string()).unwrap();
    for slug in [OPEN, STRAY] {
        std::fs::create_dir_all(home.join("moor-pool").join(slug).join("lease")).unwrap();
    }
    let machine = moor_machine_agent::Machine {
        home: home.to_owned(),
        shell: "/bin/sh".into(),
        hooks: home.join("hooks"),
        pool: Some(pool),
        allow_exec: false,
        sweep_grace: grace,
    };
    linked_machine(w, machine).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sweep_stops_a_stray_lease_and_removes_its_dir_but_not_a_kept_one() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let h = home.path();
    let _agent = host(&w, h, Duration::ZERO, None).await;
    let swept = swept(ask(&w, &json!({"op": "sweep", "keep": [7]})).await.1);
    assert_eq!((swept.removed, swept.errors), (vec![8], vec![]));
    let log = fake_crabbox::calls(h);
    assert!(
        log.contains(&format!(" stop --provider fake --id {STRAY}\n")),
        "{log}"
    );
    assert!(!log.contains(&format!("--id {OPEN}")), "{log}");
    assert!(log.ends_with(" cleanup --provider fake\n"), "{log}");
    assert!(!h.join("moor-pool").join(STRAY).exists());
    assert!(h.join("moor-pool").join(OPEN).is_dir());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sweep_spares_a_stray_lease_younger_than_the_grace() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let h = home.path();
    let _agent = host(&w, h, Duration::from_secs(3600), None).await;
    let swept = swept(ask(&w, &json!({"op": "sweep", "keep": []})).await.1);
    assert_eq!((swept.removed, swept.errors), (vec![], vec![]));
    assert!(!fake_crabbox::calls(h).contains(" stop "));
    assert!(h.join("moor-pool").join(STRAY).is_dir());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_that_fails_is_in_the_reply() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let h = home.path();
    let _agent = host(&w, h, Duration::ZERO, None).await;
    std::fs::write(h.join("stop-fail"), "").unwrap();
    let swept = swept(ask(&w, &json!({"op": "sweep", "keep": [7]})).await.1);
    assert_eq!(swept.removed, Vec::<i64>::new());
    assert_eq!(swept.errors.len(), 1, "{swept:?}");
    assert_eq!(swept.errors[0].box_id, 8);
    assert!(swept.errors[0].reason.contains("provider exploded"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cache_cap_reaches_every_crabbox_call_when_set_and_only_then() {
    for (gb, want) in [(Some(5), "5"), (None, "unset")] {
        let (w, home) = (world().await, tempfile::tempdir().unwrap());
        let h = home.path();
        let _agent = host(&w, h, Duration::ZERO, gb).await;
        ask(&w, &json!({"op": "sweep", "keep": [7]})).await;
        let warmup = json!({"op": "lease", "verb": "warmup", "slug": OPEN});
        ask(&w, &warmup).await;
        let caps = std::fs::read_to_string(h.join("cap.log")).unwrap();
        let calls = caps
            .lines()
            .filter(|l| !l.starts_with("providers "))
            .collect::<Vec<_>>();
        assert!(calls.len() >= 4, "{caps}");
        for call in calls {
            assert!(call.ends_with(&format!(" {want}")), "{gb:?}: {caps}");
        }
    }
}
