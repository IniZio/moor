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

#[tokio::test(flavor = "multi_thread")]
async fn a_pool_host_serves_checkpoint_verbs_over_the_link() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let _agent = host(&w, home.path(), false).await;
    let create = json!({"op": "checkpoint",
        "act": {"verb": "checkpoint-create", "slug": SLUG, "name": "n"}});
    let reply = serde_json::to_value(ask(&w, &create).await.1).unwrap();
    assert_eq!(reply["answer"], "checkpoint", "{reply}");
    assert_eq!(reply["id"], "chk_1", "{reply}");
    let disk = json!({"op": "checkpoint", "act": {"verb": "disk"}});
    let reply = serde_json::to_value(ask(&w, &disk).await.1).unwrap();
    assert_eq!(reply["answer"], "disk", "{reply}");
}

/// The `timing` a fork answers over the link when crabbox says `stderr`.
async fn fork_timing(stderr: Option<&str>) -> Vec<String> {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let _agent = host(&w, home.path(), false).await;
    if let Some(stderr) = stderr {
        std::fs::write(home.path().join("fork.stderr"), stderr).unwrap();
    }
    let fork = json!({"op": "checkpoint",
        "act": {"verb": "checkpoint-fork", "id": "chk_1", "slug": SLUG}});
    let reply = serde_json::to_value(ask(&w, &fork).await.1).unwrap();
    assert_eq!(reply["answer"], "forked", "{reply}");
    serde_json::from_value(reply["timing"].clone()).unwrap()
}

fn step_names(timing: &[String]) -> Vec<&str> {
    timing
        .iter()
        .map(|l| l.split(' ').nth(1).unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fork_answers_its_timing_lines_and_drops_all_else_crabbox_said() {
    let said = "waiting for ssh\n\
                timing root-flatten ms=7641 bytes=5\n\
                timing broken ms=soon\n\
                timing fork-total ms=24115\n";
    let timing = fork_timing(Some(said)).await;
    assert_eq!(
        step_names(&timing),
        ["agent-room", "agent-crabbox", "root-flatten", "fork-total"],
        "{timing:?}"
    );
    assert_eq!(timing[2], "timing root-flatten ms=7641 bytes=5");
    let crabbox: u64 = timing[1].rsplit('=').next().unwrap().parse().unwrap();
    assert!(crabbox < 60_000, "{timing:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fork_from_a_provider_with_no_timing_lines_answers_only_the_agents() {
    let timing = fork_timing(None).await;
    assert_eq!(step_names(&timing), ["agent-room", "agent-crabbox"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fork_caps_the_lines_it_sends_and_keeps_the_agents_own() {
    let flood = "timing n ms=1\n".repeat(moor_core::domain::event::MAX_STEPS * 2);
    let timing = fork_timing(Some(&flood)).await;
    assert_eq!(timing.len(), moor_core::domain::event::MAX_STEPS);
    assert_eq!(step_names(&timing)[..2], ["agent-room", "agent-crabbox"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_that_is_no_pool_host_refuses_checkpoint_verbs() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let h = home.path();
    let machine = moor_machine_agent::Machine {
        home: h.to_owned(),
        shell: "/bin/sh".into(),
        hooks: h.join("hooks"),
        pool: None,
        allow_exec: false,
        sweep_grace: Duration::ZERO,
    };
    let _agent = linked_machine(&w, machine).await;
    let disk = json!({"op": "checkpoint", "act": {"verb": "disk"}});
    let Reply::Refused { reason } = ask(&w, &disk).await.1 else {
        panic!("not refused");
    };
    assert!(reason.contains("does not serve checkpoint"), "{reason}");
}
