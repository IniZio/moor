use super::*;

#[test]
fn capacity_names_limit_and_holder() {
    let e = DriverError::Capacity {
        limit: Some(2),
        holder: Some("alice".into()),
    };
    assert_eq!(
        e.to_string(),
        "box capacity reached (limit 2), held by alice"
    );
}

#[test]
fn capacity_without_details() {
    let e = DriverError::Capacity {
        limit: None,
        holder: None,
    };
    assert_eq!(e.to_string(), "box capacity reached");
}

#[test]
fn failed_shows_only_summary() {
    let e = DriverError::Failed {
        summary: "boom".into(),
    };
    assert_eq!(e.to_string(), "boom");
}

#[test]
fn timeout_shows_seconds() {
    let e = DriverError::Timeout(Duration::from_secs(30));
    assert_eq!(e.to_string(), "box operation timed out after 30s");
}

#[test]
fn driver_is_object_safe() {
    fn _takes(_: &dyn Driver) {}
}

#[test]
fn macos_target_is_macos() {
    assert_eq!(Platform::from_target(Some("macos")), Platform::Macos);
    assert_eq!(Platform::from_target(Some("  MacOS \n")), Platform::Macos);
}

#[test]
fn other_or_missing_target_is_linux() {
    assert_eq!(Platform::from_target(Some("linux")), Platform::Linux);
    assert_eq!(Platform::from_target(Some("")), Platform::Linux);
    assert_eq!(Platform::from_target(None), Platform::Linux);
    assert_eq!(Platform::default(), Platform::Linux);
}
