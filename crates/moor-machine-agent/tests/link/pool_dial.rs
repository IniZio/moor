//! A dial into a pool lease's VM, bounded by the caller's timeout.

use super::*;

const SLUG: &str = "moor-1a2b3c4d-7";

/// A pool host on `home` whose crabbox hangs on `ssh` when `slow`.
async fn host(w: &World, home: &Path, slow: bool) -> tempfile::TempDir {
    let pool = moor_machine_agent::Pool {
        provider: "fake".to_owned(),
        max: None,
        reserve_mib: None,
        cache_gb: None,
        crabbox: fake_crabbox::install(home),
    };
    if slow {
        std::fs::write(home.join("slow-ssh"), "").unwrap();
    }
    std::fs::create_dir_all(home.join("moor-pool").join(SLUG).join("lease")).unwrap();
    let machine = moor_machine_agent::Machine {
        home: home.to_owned(),
        shell: "/bin/sh".into(),
        hooks: home.join("hooks"),
        pool: Some(pool),
        allow_exec: false,
        sweep_grace: Duration::ZERO,
    };
    linked_machine(w, machine).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dial_with_a_timeout_gives_up_at_it_rather_than_the_crabbox_limit() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let _agent = host(&w, home.path(), true).await;
    let started = std::time::Instant::now();
    let dial = json!({"op": "dial", "slug": SLUG, "timeout_secs": 1});
    let reply = ask(&w, &dial).await.1;
    assert!(started.elapsed() < Duration::from_secs(10), "{reply:?}");
    let Reply::Refused { reason } = reply else {
        panic!("{reply:?}")
    };
    assert_eq!(reason, "crabbox ssh ran past 1s");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dial_without_a_timeout_reaches_the_vm_as_before() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let _agent = host(&w, home.path(), false).await;
    let reply = ask(&w, &json!({"op": "dial", "slug": SLUG})).await.1;
    assert_eq!(reply, Reply::Started);
}
