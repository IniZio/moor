//! A Box's private cache volume, saved at sleep and found by the next Box.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::missing_docs_in_private_items,
    reason = "tests assert on fixed fixtures"
)]

use moor_client::link::lease::{Cache, CacheVolume};

use super::*;

const TOOLS_GROUP: &str = "3b3b3b3b3b3b3b3b";
const TOOLS_CANONICAL: &str = "moor-cache-3b3b3b3b3b3b3b3b-tools-0";

fn open_with_tools(slug: &str) -> BoxOp {
    BoxOp::Open {
        slug: s(slug),
        class: Some(s("small")),
        cache: Some(Cache {
            volumes: vec![CacheVolume {
                group: s(TOOLS_GROUP),
                name: s("tools"),
                key: format!("{TOOLS_GROUP}-tools"),
                paths: vec![s("~/.local/bin")],
                size_gb: 0,
            }],
            scope: None,
        }),
    }
}

#[tokio::test]
#[ignore = "needs a Docker daemon and the Box images"]
async fn a_private_cache_is_saved_at_sleep_and_a_later_box_starts_from_it() {
    sweep();
    docker(&["volume", "rm", "-f", TOOLS_CANONICAL]);
    let _cleanup = Cleanup;
    let home = tempfile::tempdir().unwrap();
    let rig = rig(home.path());

    step("first Box writes its tools from the agent place");
    let said = answers(&rig, &open_with_tools(MAIN)).await;
    assert!(
        said.contains(&BoxAnswer::Note {
            line: s("cache volumes: 0 warm, 1 cold")
        }),
        "{said:?}"
    );
    let write = "echo built > ~/.local/bin/tool && stat -c %U ~/.local ~/.local/bin";
    let (out, exit) = run(&rig, &exec(MAIN, Place::Agent, write, None, false)).await;
    assert_eq!((out.as_str(), exit), ("agent\nagent\n", Exit::Code(0)));

    step("sleep saves the private copy");
    assert_eq!(
        one(&rig, &BoxOp::Sleep { slug: s(MAIN) }).await,
        BoxAnswer::Done
    );
    let agent_image = image("MOOR_TEST_AGENT_IMAGE");
    let saved = std::process::Command::new("docker")
        .args(["run", "--rm", "--entrypoint", "cat", "-v"])
        .arg(format!("{TOOLS_CANONICAL}:/v:ro"))
        .args([agent_image.as_str(), "/v/tool"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&saved.stdout), "built\n");

    step("a second Box of the same owner and repo starts warm");
    let said = answers(&rig, &open_with_tools(OUTLIVING)).await;
    assert!(
        said.contains(&BoxAnswer::Note {
            line: s("cache volumes: 1 warm, 0 cold")
        }),
        "{said:?}"
    );
    let (out, _) = run(
        &rig,
        &exec(OUTLIVING, Place::Work, "cat ~/.local/bin/tool", None, false),
    )
    .await;
    assert_eq!(out, "built\n");

    step("sleeping twice and closing after a sleep keep the save");
    assert_eq!(
        one(&rig, &BoxOp::Sleep { slug: s(OUTLIVING) }).await,
        BoxAnswer::Done
    );
    assert_eq!(
        one(&rig, &BoxOp::Sleep { slug: s(OUTLIVING) }).await,
        BoxAnswer::Done
    );
    for slug in [MAIN, OUTLIVING] {
        assert_eq!(
            one(&rig, &BoxOp::Close { slug: s(slug) }).await,
            BoxAnswer::Done
        );
    }
    assert!(docker(&["volume", "inspect", TOOLS_CANONICAL]));
    docker(&["volume", "rm", "-f", TOOLS_CANONICAL]);
}
