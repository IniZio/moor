//! An owner's opt-out in crabbox's config is the reason the offer gives. One test alone in its own
//! process, since it points this process's environment at the config.
#![allow(
    clippy::unwrap_used,
    clippy::missing_docs_in_private_items,
    reason = "tests"
)]

use moor_machine_agent::pool::{self, Pool};
use moor_machine_agent::Machine;

#[allow(dead_code, reason = "shared with the other pool tests")]
mod fake_crabbox;

#[tokio::test]
async fn the_offer_names_the_owners_false_and_the_file_it_is_in() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("mine.yaml");
    std::fs::write(
        &config,
        "external:\n  capabilities:\n    checkpoints: false\n",
    )
    .unwrap();
    std::env::set_var("CRABBOX_CONFIG", &config);
    let machine = Machine {
        home: home.path().to_owned(),
        pool: Some(Pool {
            provider: "external".to_owned(),
            max: None,
            reserve_mib: None,
            cache_gb: None,
            crabbox: fake_crabbox::install(home.path()),
        }),
        ..Machine::from_env()
    };
    let offer = pool::offer(&machine).await.unwrap();
    assert!(!offer.checkpoints);
    let off = offer.checkpoints_off.unwrap();
    assert!(off.contains("capabilities.checkpoints is false"), "{off}");
    assert!(off.contains("mine.yaml"), "{off}");
}
