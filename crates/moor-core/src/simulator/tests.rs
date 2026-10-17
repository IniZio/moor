use super::*;
use crate::name::TaskName;

/// A trimmed inventory: unordered device types, an unavailable runtime, an iPad type.
const FIXTURE: &str = r#"{
  "devicetypes": [
    {"productFamily":"iPhone","identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17e","name":"iPhone 17e","minRuntimeVersion":1769472,"minRuntimeVersionString":"26.3.0","bundlePath":"/x"},
    {"productFamily":"iPad","identifier":"com.apple.CoreSimulator.SimDeviceType.iPad-Pro-13-inch-M5","name":"iPad Pro 13-inch (M5)","minRuntimeVersion":1769472},
    {"productFamily":"iPhone","identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro","name":"iPhone 17 Pro","minRuntimeVersion":1703936},
    {"productFamily":"iPhone","identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17","name":"iPhone 17","minRuntimeVersion":1703936},
    {"productFamily":"Apple Watch","identifier":"com.apple.CoreSimulator.SimDeviceType.Apple-Watch-11","name":"Apple Watch 11","minRuntimeVersion":2000000},
    {"productFamily":"iPhone","identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-6s","name":"iPhone 6s","minRuntimeVersion":589824}
  ],
  "runtimes": [
    {"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-26-0","version":"26.0","name":"iOS 26.0","isAvailable":true,"platform":"iOS","buildversion":"1",
     "supportedDeviceTypes":[
       {"identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-6s","name":"iPhone 6s","productFamily":"iPhone"},
       {"identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17","name":"iPhone 17","productFamily":"iPhone"}]},
    {"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-26-4","version":"26.4.1","name":"iOS 26.4","isAvailable":true,"platform":"iOS",
     "supportedDeviceTypes":[
       {"identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-6s","name":"iPhone 6s","productFamily":"iPhone"},
       {"identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17e","name":"iPhone 17e","productFamily":"iPhone"},
       {"identifier":"com.apple.CoreSimulator.SimDeviceType.iPad-Pro-13-inch-M5","name":"iPad Pro 13-inch (M5)","productFamily":"iPad"},
       {"identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro","name":"iPhone 17 Pro","productFamily":"iPhone"},
       {"identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17","name":"iPhone 17","productFamily":"iPhone"},
       {"identifier":"com.apple.CoreSimulator.SimDeviceType.Apple-Watch-11","name":"Apple Watch 11","productFamily":"Apple Watch"}]},
    {"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-27-0","version":"27.0","name":"iOS 27.0","isAvailable":false,"platform":"iOS","supportedDeviceTypes":[]},
    {"identifier":"com.apple.CoreSimulator.SimRuntime.watchOS-26-4","version":"26.4","name":"watchOS 26.4","isAvailable":true,"platform":"watchOS","supportedDeviceTypes":[]}
  ]
}"#;

const IOS_26_4: &str = "com.apple.CoreSimulator.SimRuntime.iOS-26-4";
const IOS_26_0: &str = "com.apple.CoreSimulator.SimRuntime.iOS-26-0";
const PHONE_17: &str = "com.apple.CoreSimulator.SimDeviceType.iPhone-17";
const PHONE_17E: &str = "com.apple.CoreSimulator.SimDeviceType.iPhone-17e";
const PHONE_17_PRO: &str = "com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro";

fn inv() -> Inventory {
    parse_inventory(FIXTURE).unwrap()
}

#[test]
fn list_script_is_the_simctl_listing() {
    assert_eq!(LIST_SCRIPT, "xcrun simctl list -j devicetypes runtimes");
}

#[test]
fn parse_rejects_non_json() {
    assert!(parse_inventory("not json")
        .unwrap_err()
        .contains("inventory"));
}

#[test]
fn parse_tolerates_missing_optional_fields() {
    let i = parse_inventory(
        r#"{"runtimes":[{"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-18-0","name":"iOS 18.0","version":"18.0",
        "supportedDeviceTypes":[{"identifier":"x.iPhone-16","name":"iPhone 16","productFamily":"iPhone"}]}]}"#,
    )
    .unwrap();
    let c = choose(&i, None, None).unwrap();
    assert_eq!(c.device_type, "x.iPhone-16");
}

#[test]
fn defaults_to_newest_runtime_and_newest_iphone() {
    let c = choose(&inv(), None, None).unwrap();
    assert_eq!(c.runtime, IOS_26_4);
    assert_eq!(c.device_type, PHONE_17E);
}

#[test]
fn newest_iphone_tie_goes_to_first_listed() {
    let c = choose(&inv(), None, Some("iOS 26.0")).unwrap();
    assert_eq!(c.runtime, IOS_26_0);
    assert_eq!(c.device_type, PHONE_17);
    let mut i = inv();
    i.devicetypes.retain(|d| d.identifier != PHONE_17E);
    let c = choose(&i, None, None).unwrap();
    assert_eq!(c.device_type, PHONE_17_PRO);
}

#[test]
fn runtime_versions_compare_numerically() {
    assert!(cmp_version("26.4.1", "26.0").is_gt());
    assert!(cmp_version("26.10", "26.9").is_gt());
    assert!(cmp_version("26.0", "26.0.0").is_eq());
}

#[test]
fn unavailable_and_non_ios_runtimes_are_skipped() {
    let c = choose(&inv(), None, None).unwrap();
    assert_eq!(c.runtime, IOS_26_4);
}

#[test]
fn runtime_pin_matches_name_or_identifier() {
    assert_eq!(
        choose(&inv(), None, Some("iOS 26.0")).unwrap().runtime,
        IOS_26_0
    );
    assert_eq!(
        choose(&inv(), None, Some(IOS_26_0)).unwrap().runtime,
        IOS_26_0
    );
}

#[test]
fn unknown_runtime_pin_lists_what_the_mac_has() {
    let e = choose(&inv(), None, Some("iOS 99")).unwrap_err();
    assert_eq!(e.path(), "box.simulator.runtime");
    assert_eq!(
        e.reason(),
        "unknown simulator runtime \"iOS 99\"; this Mac has: iOS 26.0, iOS 26.4"
    );
}

#[test]
fn unavailable_runtime_pin_is_unknown() {
    let e = choose(&inv(), None, Some("iOS 27.0")).unwrap_err();
    assert!(matches!(e, PinError::Runtime(_)));
}

#[test]
fn no_available_ios_runtime_is_a_runtime_error() {
    let mut i = inv();
    i.runtimes.retain(|r| !r.is_ios() || !r.available);
    let e = choose(&i, None, None).unwrap_err();
    assert_eq!(e.path(), "box.simulator.runtime");
    assert!(e.reason().contains("no available iOS"));
}

#[test]
fn device_pin_matches_name_or_identifier_and_allows_ipad() {
    let c = choose(&inv(), Some("iPhone 17"), None).unwrap();
    assert_eq!(
        (c.device_type.as_str(), c.runtime.as_str()),
        (PHONE_17, IOS_26_4)
    );
    let c = choose(&inv(), Some(PHONE_17_PRO), None).unwrap();
    assert_eq!(c.device_type, PHONE_17_PRO);
    let c = choose(&inv(), Some("iPad Pro 13-inch (M5)"), None).unwrap();
    assert!(c.device_type.contains("iPad-Pro"));
}

#[test]
fn unsupported_device_pin_lists_supported_names() {
    let e = choose(&inv(), Some("iPhone 17e"), Some("iOS 26.0")).unwrap_err();
    assert_eq!(e.path(), "box.simulator.device_type");
    assert!(
        e.reason().contains("iPhone 6s, iPhone 17"),
        "{}",
        e.reason()
    );
    assert!(!e.reason().contains("Watch"));
}

#[test]
fn watch_device_pin_is_refused() {
    let e = choose(&inv(), Some("Apple Watch 11"), None).unwrap_err();
    assert!(matches!(e, PinError::DeviceType(_)));
}

#[test]
fn device_name_prefixes_the_key() {
    let key = TaskKey::new(&TaskName::parse("fix-login").unwrap());
    assert_eq!(device_name(&key), "moor-fix-login");
}

#[test]
fn create_script_quotes_every_argument() {
    let set = BoxPath::restore("/Users/x/it's/sim").unwrap();
    let c = Choice {
        device_type: PHONE_17.into(),
        runtime: IOS_26_4.into(),
    };
    assert_eq!(
        create_script(&set, "moor-a", &c),
        format!(
            "xcrun simctl --set '/Users/x/it'\\''s/sim' create 'moor-a' '{PHONE_17}' '{IOS_26_4}'"
        )
    );
}

#[test]
fn udid_is_the_last_nonempty_line() {
    let u = "6B2C9F1E-0A4D-4E3B-9C71-1D2E3F4A5B6C";
    assert_eq!(parse_udid(&format!("warning\n{u}\n\n")), Some(u.into()));
    assert_eq!(parse_udid(&format!("  {u}  ")), Some(u.into()));
}

#[test]
fn udid_rejects_other_shapes() {
    assert_eq!(parse_udid(""), None);
    assert_eq!(parse_udid("6b2c9f1e-0a4d-4e3b-9c71-1d2e3f4a5b6c"), None);
    assert_eq!(parse_udid("6B2C9F1E-0A4D-4E3B-9C71-1D2E3F4A5B6"), None);
    assert_eq!(
        parse_udid("6B2C9F1E-0A4D-4E3B-9C71-1D2E3F4A5B6C\nerror: x"),
        None
    );
}
