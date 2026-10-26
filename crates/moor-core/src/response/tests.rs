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
        detail: None,
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
        detail: None,
    };
    assert_eq!(Response::err("x", e).class, ExitClass::Failed);
}

#[test]
fn constructors_roundtrip() {
    let e = ErrorView {
        kind: ErrorKind::Refused,
        message: "no".into(),
        hints: vec!["h".into()],
        context: vec!["c".into()],
        detail: None,
    };
    let w = Warning {
        code: "c".into(),
        message: "m".into(),
    };
    for r in [
        Response::ok("version", None),
        Response::ok("x", Some(json!({"a":1}))).with_warnings(vec![w]),
        Response::err("close", e),
    ] {
        let back: Response = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }
}

#[test]
fn wrong_schema_rejected() {
    let mut v = serde_json::to_value(Response::ok("x", None)).unwrap();
    v.as_object_mut()
        .unwrap()
        .insert("schema".into(), json!("moor/2"));
    assert!(serde_json::from_value::<Response>(v).is_err());
}

#[test]
fn error_detail_round_trips_through_json() {
    use super::CloseRefusal as C;
    for kind in [
        C::AlreadyClosed,
        C::Unsaved {
            branch: "moor/t".into(),
            ahead: 2,
            dirty: 1,
        },
        C::Unchecked {
            reason: "ssh timed out".into(),
        },
    ] {
        let detail = ErrorDetail::CloseRefused(kind);
        let text = serde_json::to_string(&detail).unwrap();
        assert_eq!(serde_json::from_str::<ErrorDetail>(&text).unwrap(), detail);
    }
    let json = serde_json::to_value(ErrorDetail::CloseRefused(C::AlreadyClosed)).unwrap();
    assert_eq!(
        json,
        json!({"kind":"close_refused","refusal":"already_closed"})
    );
}

#[test]
fn error_without_detail_reads_and_writes_as_before() {
    let old = json!({"kind":"refused","message":"no","hints":[],"context":[]});
    let e: ErrorView = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(e.detail, None);
    assert_eq!(serde_json::to_value(&e).unwrap(), old);
}
