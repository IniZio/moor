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
fn platform_parses_os_and_arch() {
    let p = BoxPlatform::parse("Darwin arm64\n").unwrap();
    assert_eq!((p.os.as_str(), p.arch.as_str()), ("Darwin", "arm64"));
}

#[test]
fn platform_rejects_wrong_word_count() {
    assert_eq!(BoxPlatform::parse(""), None);
    assert_eq!(BoxPlatform::parse("Linux"), None);
    assert_eq!(BoxPlatform::parse("Linux x86_64 extra"), None);
}
