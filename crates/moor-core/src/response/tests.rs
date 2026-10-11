//! Tests for the response envelope.

use super::*;
use serde_json::json;

#[test]
fn ok_envelope_keys_and_nulls() {
    let v = serde_json::to_value(Response::ok("version", None)).unwrap();
    assert_eq!(
        v,
        json!({"schema":"moor/1","cmd":"version","class":"ok","value":null,"warnings":[],"error":null})
    );
    assert_eq!(v.as_object().unwrap().len(), 6);
}

#[test]
fn ok_with_value_and_warnings() {
    let w = Warning {
        code: "c".into(),
        message: "m".into(),
    };
    let v = serde_json::to_value(Response::ok("x", Some(json!({"a":1}))).with_warnings(vec![w]))
        .unwrap();
    assert_eq!(v.get("value"), Some(&json!({"a":1})));
    assert_eq!(
        v.get("warnings"),
        Some(&json!([{"code":"c","message":"m"}]))
    );
}

#[test]
fn err_envelope() {
    let e = ErrorView {
        kind: ErrorKind::Refused,
        message: "no".into(),
        hints: vec!["h".into()],
        context: vec![],
    };
    let v = serde_json::to_value(Response::err("close", e)).unwrap();
    assert_eq!(v.as_object().unwrap().len(), 6);
    assert_eq!(v.get("class"), Some(&json!("refused")));
    assert_eq!(v.get("value"), Some(&serde_json::Value::Null));
    assert_eq!(
        v.get("error"),
        Some(&json!({"kind":"refused","message":"no","hints":["h"],"context":[]}))
    );
}

#[test]
fn class_strings_lowercase() {
    for (c, s) in [
        (ExitClass::Ok, "ok"),
        (ExitClass::Failed, "failed"),
        (ExitClass::Usage, "usage"),
        (ExitClass::Refused, "refused"),
    ] {
        assert_eq!(serde_json::to_value(c).unwrap(), s);
    }
}

#[test]
fn kind_maps_to_class() {
    assert_eq!(ExitClass::from(ErrorKind::Usage), ExitClass::Usage);
    assert_eq!(ExitClass::from(ErrorKind::Refused), ExitClass::Refused);
    assert_eq!(ExitClass::from(ErrorKind::Failed), ExitClass::Failed);
    assert_eq!(ExitClass::from(ErrorKind::Internal), ExitClass::Failed);
    let e = ErrorView {
        kind: ErrorKind::Internal,
        message: "b".into(),
        hints: vec![],
        context: vec![],
    };
    assert_eq!(Response::err("x", e).class, ExitClass::Failed);
}
