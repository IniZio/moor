//! A pool host's checkpoint verbs and what it offers, against a fake crabbox.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_docs_in_private_items,
    reason = "tests"
)]

use std::path::Path;

use moor_client::link::lease::{Answer, Cache, CacheVolume, CheckpointAct};
use moor_client::link::ops::Reply;
use moor_machine_agent::pool::{self, Pool};
use moor_machine_agent::Machine;

#[allow(dead_code, reason = "shared with the other pool tests")]
mod fake_crabbox;

const SLUG: &str = "moor-1a2b3c4d-7";
const BOTH: &str = r#"{"capabilities":{"features":["workspace-checkpoint","workspace-fork"]}}"#;

fn on(home: &Path) -> Pool {
    Pool {
        provider: "fake".to_owned(),
        max: None,
        reserve_mib: None,
        cache_gb: None,
        crabbox: fake_crabbox::install(home),
    }
}

fn host() -> (tempfile::TempDir, Pool) {
    let home = tempfile::tempdir().unwrap();
    let pool = on(home.path());
    (home, pool)
}

fn make_lease_dir(home: &Path) -> std::path::PathBuf {
    let dir = home.join("moor-pool").join(SLUG).join("lease");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn create(name: &str) -> CheckpointAct {
    CheckpointAct::CheckpointCreate {
        slug: SLUG.to_owned(),
        name: name.to_owned(),
    }
}

fn fork(id: &str) -> CheckpointAct {
    CheckpointAct::CheckpointFork {
        id: id.to_owned(),
        slug: SLUG.to_owned(),
        cache: None,
        class: None,
    }
}

fn refused(reply: &Reply) -> &str {
    match reply {
        Reply::Refused { reason } => reason,
        other => panic!("not refused: {other:?}"),
    }
}

#[tokio::test]
async fn a_create_runs_native_in_the_lease_dir_and_answers_the_delta_as_data() {
    let (home, pool) = host();
    let dir = make_lease_dir(home.path());
    let reply = pool::checkpoint_op(home.path(), &pool, create("layer-1")).await;
    assert_eq!(
        reply,
        Reply::Leased(Answer::Checkpoint {
            id: "chk_1".to_owned(),
            bytes: Some(5),
            parent: Some("chk_0".to_owned()),
        })
    );
    let log = fake_crabbox::calls(home.path());
    assert_eq!(
        log.trim(),
        format!(
            "{} checkpoint create --provider fake --id {SLUG} --name layer-1 --mode native --json",
            dir.display()
        )
    );
}

#[tokio::test]
async fn a_create_of_a_lease_this_host_lacks_runs_nothing() {
    let (home, pool) = host();
    let reply = pool::checkpoint_op(home.path(), &pool, create("layer-1")).await;
    assert!(refused(&reply).contains("no lease"));
    assert_eq!(fake_crabbox::calls(home.path()), "");
}

#[tokio::test]
async fn a_word_that_could_be_a_flag_or_a_path_never_reaches_crabbox() {
    let (home, pool) = host();
    make_lease_dir(home.path());
    let bad_slug = CheckpointAct::CheckpointCreate {
        slug: "../../etc".to_owned(),
        name: "n".to_owned(),
    };
    let bad_class = CheckpointAct::CheckpointFork {
        id: "chk_1".to_owned(),
        slug: SLUG.to_owned(),
        cache: None,
        class: Some("--evil".to_owned()),
    };
    let acts = [
        create("--mode=archive"),
        create("a/b"),
        create(""),
        create(&"a".repeat(129)),
        fork("--dry-run"),
        fork("a b"),
        CheckpointAct::CheckpointDelete {
            id: "-rf".to_owned(),
        },
        bad_slug,
        bad_class,
    ];
    for act in acts {
        let reply = pool::checkpoint_op(home.path(), &pool, act.clone()).await;
        refused(&reply);
        assert_eq!(fake_crabbox::calls(home.path()), "", "{act:?}");
    }
}

#[tokio::test]
async fn a_create_answer_holding_an_unsendable_id_is_refused() {
    let (home, pool) = host();
    make_lease_dir(home.path());
    for record in [
        r#"{"id":"../x"}"#,
        r#"{"id":"chk_1","native":{"parentCheckpointId":"--x"}}"#,
        "not json",
    ] {
        std::fs::write(home.path().join("create.json"), record).unwrap();
        let reply = pool::checkpoint_op(home.path(), &pool, create("n")).await;
        refused(&reply);
    }
}

#[tokio::test]
async fn a_fork_boots_in_a_new_lease_dir_with_its_cache_and_class() {
    let (home, pool) = host();
    let act = CheckpointAct::CheckpointFork {
        id: "chk_1".to_owned(),
        slug: SLUG.to_owned(),
        cache: Some(Cache {
            volumes: vec![CacheVolume {
                group: "g".to_owned(),
                name: "n".to_owned(),
                key: "g-n".to_owned(),
                paths: vec!["~/c".to_owned()],
                size_gb: 0,
            }],
            scope: Some("0123456789abcdef".to_owned()),
        }),
        class: Some("fast".to_owned()),
    };
    let reply = pool::checkpoint_op(home.path(), &pool, act).await;
    assert!(
        matches!(reply, Reply::Leased(Answer::Forked { .. })),
        "{reply:?}"
    );
    let dir = home.path().join("moor-pool").join(SLUG).join("lease");
    assert!(dir.is_dir());
    let log = fake_crabbox::calls(home.path());
    assert!(
        log.contains(&format!(
            "{} checkpoint fork chk_1 --slug {SLUG} --class fast --json\n",
            dir.display()
        )),
        "{log}"
    );
    let env = std::fs::read_to_string(home.path().join("cache.env")).unwrap();
    assert!(env.contains("0123456789abcdef"), "{env}");
}

#[tokio::test]
async fn a_fork_that_fails_stops_what_it_started_and_leaves_no_dir() {
    let (home, pool) = host();
    std::fs::write(home.path().join("fork-fail"), "").unwrap();
    let reply = pool::checkpoint_op(home.path(), &pool, fork("chk_1")).await;
    assert!(refused(&reply).contains("network exploded"));
    let log = fake_crabbox::calls(home.path());
    assert!(
        log.contains(&format!("stop --provider fake --id {SLUG}\n")),
        "{log}"
    );
    assert!(!home.path().join("moor-pool").join(SLUG).exists());
}

#[tokio::test]
async fn a_fork_that_fails_on_a_lease_already_there_leaves_it() {
    let (home, pool) = host();
    make_lease_dir(home.path());
    std::fs::write(home.path().join("fork-fail"), "").unwrap();
    refused(&pool::checkpoint_op(home.path(), &pool, fork("chk_1")).await);
    assert!(!fake_crabbox::calls(home.path()).contains(" stop "));
    assert!(home.path().join("moor-pool").join(SLUG).exists());
}

#[tokio::test]
async fn a_fork_naming_another_lease_is_undone() {
    let (home, pool) = host();
    std::fs::write(home.path().join("fork.json"), r#"{"slug":"elsewhere"}"#).unwrap();
    let reply = pool::checkpoint_op(home.path(), &pool, fork("chk_1")).await;
    assert!(refused(&reply).contains("elsewhere"));
    assert!(!home.path().join("moor-pool").join(SLUG).exists());
}

#[tokio::test]
async fn a_fork_past_the_pools_most_leases_runs_nothing_of_the_fork() {
    let (home, mut pool) = host();
    pool.max = Some(1);
    let leases = r#"[{"slug":"moor-1a2b3c4d-8","state":"running"}]"#;
    std::fs::write(home.path().join("leases.json"), leases).unwrap();
    let reply = pool::checkpoint_op(home.path(), &pool, fork("chk_1")).await;
    assert!(refused(&reply).contains("most"));
    assert!(!fake_crabbox::calls(home.path()).contains("checkpoint"));
}

#[tokio::test]
async fn a_delete_counts_one_already_gone_as_deleted_and_says_any_other_failure() {
    let (home, pool) = host();
    let act = CheckpointAct::CheckpointDelete {
        id: "chk_1".to_owned(),
    };
    let reply = pool::checkpoint_op(home.path(), &pool, act.clone()).await;
    assert_eq!(reply, Reply::Leased(Answer::Done));
    assert!(fake_crabbox::calls(home.path()).contains(" checkpoint delete chk_1\n"));
    std::fs::write(home.path().join("delete-gone"), "").unwrap();
    let reply = pool::checkpoint_op(home.path(), &pool, act.clone()).await;
    assert_eq!(reply, Reply::Leased(Answer::Done));
    std::fs::write(home.path().join("delete-fail"), "").unwrap();
    let reply = pool::checkpoint_op(home.path(), &pool, act).await;
    assert!(refused(&reply).contains("provider exploded"));
}

#[tokio::test]
async fn disk_answers_the_filesystem_in_mib_and_runs_no_crabbox() {
    let (home, pool) = host();
    let reply = pool::checkpoint_op(home.path(), &pool, CheckpointAct::Disk {}).await;
    let Reply::Leased(Answer::Disk {
        free_mib,
        total_mib,
    }) = reply
    else {
        panic!("{reply:?}");
    };
    assert!(total_mib > 0 && free_mib <= total_mib);
    assert_eq!(fake_crabbox::calls(home.path()), "");
}

fn machine(home: &Path, pool: Pool) -> Machine {
    Machine {
        home: home.to_owned(),
        pool: Some(pool),
        ..Machine::from_env()
    }
}

fn describe(home: &Path, json: &str) {
    let path = home.join("describe.json");
    std::fs::write(path, json).unwrap();
}

#[tokio::test]
async fn the_offer_has_checkpoints_with_both_features_and_a_passing_doctor() {
    let (home, pool) = host();
    describe(home.path(), BOTH);
    let offer = pool::offer(&machine(home.path(), pool)).await.unwrap();
    assert!(offer.checkpoints);
    assert_eq!(offer.checkpoints_off, None);
    let log = fake_crabbox::calls(home.path());
    assert!(log.contains(" doctor --provider fake\n"), "{log}");
}

#[tokio::test]
async fn the_offer_says_which_feature_is_missing_and_runs_no_doctor() {
    for (features, lacks) in [
        (r#"["workspace-checkpoint"]"#, "workspace-fork"),
        (r#"["workspace-fork"]"#, "workspace-checkpoint"),
        ("[]", "workspace-checkpoint"),
    ] {
        let (home, pool) = host();
        let json = format!(r#"{{"capabilities":{{"features":{features}}}}}"#);
        describe(home.path(), &json);
        let offer = pool::offer(&machine(home.path(), pool)).await.unwrap();
        assert!(!offer.checkpoints, "{features}");
        let off = offer.checkpoints_off.unwrap();
        assert!(off.contains(lacks), "{off}");
        assert!(!fake_crabbox::calls(home.path()).contains(" doctor "));
    }
}

#[tokio::test]
async fn the_offer_names_the_adapters_failing_check_once_and_no_passing_one() {
    let (home, pool) = host();
    describe(home.path(), BOTH);
    std::fs::write(home.path().join("doctor-fail"), "").unwrap();
    let offer = pool::offer(&machine(home.path(), pool)).await.unwrap();
    assert!(!offer.checkpoints);
    let off = offer.checkpoints_off.unwrap();
    assert!(off.contains("v52.0-moor.1 is required"), "{off}");
    assert_eq!(off.matches("FAIL checkpoints").count(), 1, "{off}");
    assert!(!off.contains("ok kvm"), "{off}");
}

#[tokio::test]
async fn the_offer_also_names_a_failed_check_of_crabbox_itself() {
    let (home, pool) = host();
    describe(home.path(), BOTH);
    std::fs::write(home.path().join("doctor-fail"), "").unwrap();
    std::fs::write(home.path().join("doctor-config"), "").unwrap();
    let offer = pool::offer(&machine(home.path(), pool)).await.unwrap();
    let off = offer.checkpoints_off.unwrap();
    assert!(off.contains("permissions 0664 want 0600"), "{off}");
    assert!(off.contains("v52.0-moor.1 is required"), "{off}");
}
