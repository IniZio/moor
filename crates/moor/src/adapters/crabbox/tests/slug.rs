//! Tests that the crabbox slug and static id come from the Box id, not its name.

use moor_core::decide::reconcile::{reconcile, BoxFact};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::BoxTarget;
use moor_core::name::{BoxHandle, BoxId, BoxName, ProviderId};
use moor_core::seam::driver::Driver;
use moor_core::seam::proc::Tool;

use super::{fixture, probe, req, Reply, WARMUP_OUT};

#[test]
fn slug_of_longest_name_fits_crabbox_cap() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(WARMUP_OUT.to_owned()));
    probe(&p);
    let mut r = req("sprites", None);
    r.name = BoxName::parse_allow_bare(&format!("{}/{}", "r".repeat(40), "l".repeat(40))).unwrap();
    let got = c.acquire(&r).unwrap();
    let calls = p.calls();
    let args = &calls.first().unwrap().args;
    let slug = args
        .get(args.iter().position(|a| a == "--slug").unwrap() + 1)
        .unwrap();
    assert!(slug.len() <= 41, "{slug}");
    assert_eq!(slug, "moor-7-ab12cd");
    assert_eq!(got.handle.as_str(), "sprites/moor-7-ab12cd");
}

#[test]
fn static_id_of_slashed_name_has_no_slash() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(WARMUP_OUT.to_owned()));
    probe(&p);
    let mut r = req("ssh", None);
    r.name = BoxName::parse_allow_bare("web/fix-bug").unwrap();
    c.acquire(&r).unwrap();
    let calls = p.calls();
    let id = calls
        .first()
        .unwrap()
        .env
        .iter()
        .find(|(k, _)| k == "CRABBOX_STATIC_ID")
        .map(|(_, v)| v.clone())
        .unwrap();
    assert_eq!(id, "moor-7-ab12cd");
    assert!(!id.contains('/'));
}

#[test]
fn slash_named_box_stays_live_through_list_and_reconcile() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(WARMUP_OUT.to_owned()));
    probe(&p);
    let mut r = req("sprites", None);
    r.name = BoxName::parse_allow_bare("web/fix-bug").unwrap();
    let handle = c.acquire(&r).unwrap().handle;
    let json = r#"[{"id":1,"labels":{"slug":"moor-7-ab12cd"}}]"#;
    p.on(Tool::Crabbox, &["list"], Reply::Ok(json.to_owned()));
    let listing = c.list(&ProviderId::new("sprites")).unwrap();
    let rows = [
        BoxFact {
            box_id: BoxId::new(7),
            state: BoxState::Ready,
            handle: Some(handle),
        },
        BoxFact {
            box_id: BoxId::new(8),
            state: BoxState::Ready,
            handle: Some(BoxHandle::new("sprites/moor-8-ab12cd")),
        },
    ];
    let out = reconcile(&rows, &listing);
    let tasks: Vec<_> = out.iter().map(|d| d.target.clone()).collect();
    assert_eq!(tasks, [BoxTarget::Existing(BoxId::new(8))]);
}
