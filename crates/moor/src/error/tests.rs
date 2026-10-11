use super::CommandError;
use moor_core::{ErrorKind, ExitClass, Warning};

#[test]
fn not_implemented_is_failed_internal() {
    let resp = CommandError::not_implemented("close").into_response("close", vec![]);
    assert_eq!(resp.class, ExitClass::Failed);
    assert_eq!(resp.cmd, "close");
    assert!(resp.value.is_none());
    let err = resp.error.expect("error view");
    assert_eq!(err.kind, ErrorKind::Internal);
    assert!(err.message.contains("`moor close`"));
}

#[test]
fn kind_maps_to_class_and_hints_and_warnings_survive() {
    let warning = Warning {
        code: "w".into(),
        message: "careful".into(),
    };
    let resp = CommandError::new(ErrorKind::Refused, "no")
        .with_hint("try later")
        .into_response("show", vec![warning.clone()]);
    assert_eq!(resp.class, ExitClass::Refused);
    assert_eq!(resp.warnings, vec![warning]);
    let err = resp.error.expect("error view");
    assert_eq!(err.message, "no");
    assert_eq!(err.hints, vec!["try later".to_owned()]);
}

#[test]
fn usage_kind_is_usage_class() {
    let resp = CommandError::new(ErrorKind::Usage, "bad").into_response("run", vec![]);
    assert_eq!(resp.class, ExitClass::Usage);
}
