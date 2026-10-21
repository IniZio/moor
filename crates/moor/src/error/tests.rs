use super::CommandError;
use moor_core::{ErrorKind, ExitClass, Warning};

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

#[test]
fn value_rides_the_failure_envelope() {
    let resp = CommandError::new(ErrorKind::Failed, "boom")
        .with_value(serde_json::json!({"box": {"state": "setup_failed"}}))
        .into_response("start", vec![]);
    assert_eq!(resp.class, ExitClass::Failed);
    let value = resp.value.expect("value present");
    assert_eq!(
        value.pointer("/box/state"),
        Some(&serde_json::json!("setup_failed"))
    );
    let bare = CommandError::new(ErrorKind::Failed, "boom").into_response("start", vec![]);
    assert!(bare.value.is_none());
}

// --- seam conversions: driver, surface ---

mod seam_driver_surface {
    use super::CommandError;
    use moor_core::seam::driver::DriverError;
    use moor_core::seam::surface::SurfaceError;
    use moor_core::ErrorKind;
    use std::time::Duration;

    #[test]
    fn driver_capacity_is_refused_with_hint() {
        let e = CommandError::from(DriverError::Capacity {
            limit: Some(2),
            holder: None,
        });
        assert_eq!(e.kind, ErrorKind::Refused);
        assert_eq!(
            e.hints,
            vec!["retry after another Box closes; `moor show` lists holders".to_owned()]
        );
    }

    #[test]
    fn other_driver_errors_are_failed_without_hints() {
        let all = [
            DriverError::NotFound,
            DriverError::Quota("q".into()),
            DriverError::Auth("a".into()),
            DriverError::Timeout(Duration::from_secs(3)),
            DriverError::Unavailable("u".into()),
            DriverError::Unsupported("snapshot"),
            DriverError::Failed {
                summary: "s".into(),
            },
        ];
        for err in all {
            let text = err.to_string();
            let e = CommandError::from(err);
            assert_eq!(e.kind, ErrorKind::Failed);
            assert!(e.hints.is_empty());
            assert_eq!(e.error.to_string(), text);
        }
    }

    #[test]
    fn surface_errors_are_failed_with_message() {
        let all = [
            SurfaceError::NotFound,
            SurfaceError::Timeout,
            SurfaceError::Unavailable("herdr down".into()),
            SurfaceError::Failed {
                summary: "s".into(),
            },
        ];
        for err in all {
            let text = err.to_string();
            let e = CommandError::from(err);
            assert_eq!(e.kind, ErrorKind::Failed);
            assert_eq!(e.error.to_string(), text);
        }
    }
}

// --- store and close conversions ---

mod store_close {
    use super::CommandError;
    use crate::store::StoreError;
    use moor_core::decide::close::Refusal;
    use moor_core::domain::Illegal;
    use moor_core::{BoxName, ErrorKind};

    #[test]
    fn illegal_store_error_is_internal() {
        let e = CommandError::from(StoreError::Illegal(Illegal {
            from: "closed".into(),
            input: "start".into(),
        }));
        assert_eq!(e.kind, ErrorKind::Internal);
    }

    #[test]
    fn blocked_sleep_is_refused() {
        let e = CommandError::from(StoreError::SleepBlocked { busy: 2 });
        assert_eq!(e.kind, ErrorKind::Refused);
        assert_eq!(e.error.to_string(), "sleep blocked: 2 Sessions busy");
    }

    #[test]
    fn other_store_error_is_failed_with_moor_wording() {
        let e = CommandError::from(StoreError::Poisoned);
        assert_eq!(e.kind, ErrorKind::Failed);
        assert!(e.error.to_string().starts_with("state store error: "));
    }

    #[test]
    fn unpushed_work_is_refused_with_hint() {
        let e = CommandError::from(Refusal::UnpushedWork {
            unpushed: 2,
            dirty: true,
        });
        assert_eq!(e.kind, ErrorKind::Refused);
        assert_eq!(e.hints, vec!["commit and push, or pass --force".to_owned()]);
    }

    #[test]
    fn already_closed_is_refused_without_hint() {
        let e = CommandError::from(Refusal::AlreadyClosed);
        assert_eq!(e.kind, ErrorKind::Refused);
        assert_eq!(e.error.to_string(), "Box is already closed");
        assert!(e.hints.is_empty());
    }

    #[test]
    fn bad_name_is_usage() {
        let err = BoxName::parse_allow_bare("Bad Name!").expect_err("rejected");
        assert_eq!(CommandError::from(err).kind, ErrorKind::Usage);
    }
}
