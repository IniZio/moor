//! The `node-settings-*` requests on the wire.

use std::num::NonZeroU32;

use serde_json::json;

use super::{CapEdit, NodeSettingsGetReq, NodeSettingsSetReq};
use crate::request::Request;

fn set(max_boxes: CapEdit, labels: Option<Vec<String>>) -> Request {
    Request::NodeSettingsSet(NodeSettingsSetReq {
        identity: None,
        name: "box-host".into(),
        max_boxes,
        labels,
        expected_revision: None,
    })
}

#[test]
fn a_get_is_a_name_and_nothing_else() {
    let get: Request =
        serde_json::from_str(r#"{"cmd":"node-settings-get","name":"box-host"}"#).unwrap();
    assert_eq!(
        get,
        Request::NodeSettingsGet(NodeSettingsGetReq {
            identity: None,
            name: "box-host".into(),
        })
    );
    assert_eq!(get.cmd(), "node-settings-get");
    let extra = r#"{"cmd":"node-settings-get","name":"n","bogus":1}"#;
    assert!(serde_json::from_str::<Request>(extra).is_err());
}

#[test]
fn a_set_leaves_what_it_keeps_off_the_wire() {
    let keep = serde_json::to_value(set(CapEdit::Keep, None)).unwrap();
    assert_eq!(keep, json!({"cmd":"node-settings-set","name":"box-host"}));
    let cap = NonZeroU32::new(2).unwrap();
    let set_cap = serde_json::to_value(set(CapEdit::Set(cap), Some(vec!["gpu".into()]))).unwrap();
    assert_eq!(
        set_cap,
        json!({"cmd":"node-settings-set","name":"box-host","max_boxes":{"set":2},"labels":["gpu"]})
    );
    let clear = serde_json::to_value(set(CapEdit::Clear, Some(vec![]))).unwrap();
    assert_eq!(
        clear,
        json!({"cmd":"node-settings-set","name":"box-host","max_boxes":"clear","labels":[]})
    );
    for wire in [keep, set_cap, clear] {
        let back: Request = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(back).unwrap(), wire);
    }
}

#[test]
fn a_cap_of_zero_is_not_a_cap() {
    let zero = r#"{"cmd":"node-settings-set","name":"n","max_boxes":{"set":0}}"#;
    assert!(serde_json::from_str::<Request>(zero).is_err());
}
