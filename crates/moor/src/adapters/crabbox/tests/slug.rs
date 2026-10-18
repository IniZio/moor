//! Tests that the crabbox slug and static id come from the Box id, not its name.

use moor_core::name::{BoxId, BoxName, ProviderId};
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
fn list_fills_id_from_slug() {
    let (p, c) = fixture();
    let json = r#"[{"id":1,"labels":{"slug":"moor-7-a1b2c3"}},
        {"id":2,"labels":{"slug":"moor-fix-bug-a1b2c3"}}]"#;
    p.on(Tool::Crabbox, &["list"], Reply::Ok(json.to_owned()));
    let got = c.list(&ProviderId::new("sprites")).unwrap();
    assert_eq!(got.first().unwrap().id, Some(BoxId::new(7)));
    assert_eq!(got.get(1).unwrap().id, None);
}
