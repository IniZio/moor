use serde_json::json;

use super::*;
use crate::request::{Request, SteerReq};

fn parse(v: serde_json::Value) -> Result<BoxTarget, String> {
    serde_json::from_value(v).map_err(|e| e.to_string())
}

#[test]
fn a_string_is_a_name_and_an_object_is_an_id() {
    let name = BoxName::parse("repo/alpha").expect("name");
    assert_eq!(parse(json!("repo/alpha")), Ok(BoxTarget::Name(name)));
    assert_eq!(parse(json!({"id": 42})), Ok(BoxTarget::Id(BoxId::new(42))));
}

#[test]
fn both_forms_round_trip() {
    for wire in [json!("repo/alpha"), json!({"id": 7})] {
        let target = parse(wire.clone()).expect("parse");
        assert_eq!(serde_json::to_value(&target).expect("serialize"), wire);
    }
}

#[test]
fn a_bad_value_names_what_is_wrong() {
    let err = parse(json!({"name": "repo/alpha"})).expect_err("unknown key");
    assert!(err.contains("unknown field `name`"), "{err}");
    let err = parse(json!({})).expect_err("no id");
    assert!(err.contains("missing field `id`"), "{err}");
    let err = parse(json!({"id": "7"})).expect_err("string id");
    assert!(err.contains("invalid type: string"), "{err}");
    let err = parse(json!(7)).expect_err("bare number");
    assert!(err.contains("a Box name or"), "{err}");
    let err = parse(json!("Not A Name")).expect_err("bad name");
    assert!(err.contains("invalid Box name"), "{err}");
}

#[test]
fn an_id_must_be_positive() {
    for bad in [-1, 0] {
        let err = parse(json!({"id": bad})).expect_err("non-positive id");
        assert!(err.contains("positive"), "{err}");
    }
    assert_eq!(parse(json!({"id": 1})), Ok(BoxTarget::Id(BoxId::new(1))));
}

#[test]
fn a_request_takes_either_form_in_its_box_field() {
    let by_id: Request =
        serde_json::from_value(json!({"cmd": "steer", "box": {"id": 3}, "text": "hi"}))
            .expect("parse");
    assert_eq!(
        by_id,
        Request::Steer(SteerReq {
            target: BoxTarget::Id(BoxId::new(3)),
            text: Some("hi".into()),
            menu: None,
        })
    );
    let err = serde_json::from_value::<Request>(json!({"cmd": "steer", "box": {"id": 3, "x": 1}}))
        .expect_err("unknown key");
    assert!(err.to_string().contains("unknown field `x`"), "{err}");
}

#[test]
fn display_names_the_box_or_its_id() {
    let name = BoxName::parse("repo/alpha").expect("name");
    assert_eq!(BoxTarget::Name(name).to_string(), "repo/alpha");
    assert_eq!(BoxTarget::Id(BoxId::new(9)).to_string(), "id 9");
}
