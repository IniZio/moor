//! A Box whose places are down while it is meant to run is listed down, saying how they ended.

use super::*;
use crate::boxes::provider::ProvideError;

const LOST: &str = "the Docker daemon at unix:///run/user/1003/docker.sock is not answering: dial";

async fn listed(rig: &Rig) -> Vec<moor_client::link::boxes::BoxReport> {
    let list = BoxOp::List {
        controller: ControllerId::parse("1a2b3c4d").unwrap(),
    };
    let BoxAnswer::Listed { boxes } = rig.one(&list).await else {
        panic!("not listed");
    };
    boxes
}

#[tokio::test]
async fn a_box_meant_to_run_is_listed_down_when_its_provider_lost_every_box() {
    let rig = rig();
    rig.opened().await;
    rig.boxes
        .provider
        .fail_observe(ProvideError::Lost(LOST.to_owned()));
    let boxes = listed(&rig).await;
    assert_eq!(
        (boxes[0].state.clone(), boxes[0].down.as_deref()),
        (ReportState::Mending, Some(LOST))
    );
}

#[tokio::test]
async fn a_provider_that_cannot_look_is_not_a_box_that_is_down() {
    let rig = rig();
    rig.opened().await;
    rig.boxes
        .provider
        .fail_observe(ProvideError::Failed("docker: slow".to_owned()));
    let boxes = listed(&rig).await;
    assert_eq!(
        (boxes[0].state.clone(), boxes[0].down.clone()),
        (ReportState::Mending, None)
    );
}

#[tokio::test]
async fn a_box_asleep_on_request_is_not_listed_down_when_its_provider_lost_every_box() {
    let rig = rig();
    rig.opened().await;
    assert_eq!(
        rig.one(&BoxOp::Sleep { slug: s(SLUG) }).await,
        BoxAnswer::Done
    );
    rig.boxes
        .provider
        .fail_observe(ProvideError::Lost(LOST.to_owned()));
    let boxes = listed(&rig).await;
    assert_eq!(
        (boxes[0].state.clone(), boxes[0].down.clone()),
        (ReportState::Asleep, None)
    );
}

#[tokio::test]
async fn a_work_place_that_is_down_ends_exec_as_down_and_the_agent_place_runs_on() {
    let rig = rig();
    rig.opened().await;
    rig.boxes.provider.set_work_down(&slug(), true);
    let (out, _, exit) = rig.run(&exec(Place::Work, "echo never", None)).await;
    assert_eq!((out.as_str(), exit), ("", Exit::Down));
    let (out, _, exit) = rig.run(&exec(Place::Agent, "echo here", None)).await;
    assert_eq!((out.as_str(), exit), ("here\n", Exit::Code(0)));
    let listed = rig
        .one(&BoxOp::List {
            controller: ControllerId::parse("1a2b3c4d").unwrap(),
        })
        .await;
    let BoxAnswer::Listed { boxes } = listed else {
        panic!("{listed:?}");
    };
    assert_eq!(boxes[0].state, ReportState::Mending);
    assert_eq!(
        boxes[0].down.as_deref(),
        Some("the Box's container is not running")
    );
}

#[tokio::test]
async fn a_box_whose_places_died_is_listed_down_with_how_they_ended() {
    let rig = rig();
    rig.opened().await;
    let died = rig.home.path().join("fake").join(SLUG).join("died");
    std::fs::write(&died, "137 oom").unwrap();
    let list = BoxOp::List {
        controller: ControllerId::parse("1a2b3c4d").unwrap(),
    };
    let BoxAnswer::Listed { boxes } = rig.one(&list).await else {
        panic!("not listed");
    };
    assert_eq!(boxes[0].state, ReportState::Mending);
    assert_eq!(
        boxes[0].down.as_deref(),
        Some("the Box's container ran out of memory (exit 137)")
    );
    assert!(matches!(
        rig.one(&BoxOp::Wake { slug: s(SLUG) }).await,
        BoxAnswer::Woke { .. }
    ));
    let BoxAnswer::Listed { boxes } = rig.one(&list).await else {
        panic!("not listed");
    };
    assert_eq!(
        (boxes[0].state.clone(), boxes[0].down.clone()),
        (ReportState::Awake, None)
    );
}

#[tokio::test]
async fn a_box_asleep_on_request_is_never_listed_down() {
    let rig = rig();
    rig.opened().await;
    assert_eq!(
        rig.one(&BoxOp::Sleep { slug: s(SLUG) }).await,
        BoxAnswer::Done
    );
    let BoxAnswer::Listed { boxes } = rig
        .one(&BoxOp::List {
            controller: ControllerId::parse("1a2b3c4d").unwrap(),
        })
        .await
    else {
        panic!("not listed");
    };
    assert_eq!(
        (boxes[0].state.clone(), boxes[0].down.clone()),
        (ReportState::Asleep, None)
    );
}

#[tokio::test]
async fn a_box_a_verb_works_on_is_never_listed_down() {
    let rig = rig();
    rig.opened().await;
    std::fs::write(rig.home.path().join("fake").join(SLUG).join("died"), "143").unwrap();
    let held = rig.boxes.provider.hold_opens().await;
    let mut reopen = rig.ask(&open()).await;
    let said = ops::read_line::<_, BoxAnswer>(&mut reopen).await.unwrap();
    assert!(matches!(said, BoxAnswer::Note { .. }), "{said:?}");
    let BoxAnswer::Listed { boxes } = rig
        .one(&BoxOp::List {
            controller: ControllerId::parse("1a2b3c4d").unwrap(),
        })
        .await
    else {
        panic!("not listed");
    };
    assert_eq!(
        (boxes[0].state.clone(), boxes[0].down.clone()),
        (ReportState::Mending, None)
    );
    drop(held);
}
