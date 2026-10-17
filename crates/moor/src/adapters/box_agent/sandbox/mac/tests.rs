use moor_core::config::RepoSandbox;
use moor_core::mac_capability::{selected, MacCapability, Selection};
use moor_core::name::{BoxPath, TaskName};
use moor_core::task_key::{MacLayout, TaskKey};
use serde_json::{json, Value};

use super::{apply, MacInput};

const DEV: &str = "/Applications/Xcode.app/Contents/Developer";

fn path(s: &str) -> BoxPath {
    BoxPath::restore(s).unwrap()
}

fn layout() -> MacLayout {
    let key = TaskKey::new(&TaskName::parse("t").unwrap());
    MacLayout::new(&path("/Users/Shared/moor"), &key).unwrap()
}

fn run(caps: &[MacCapability], sandbox: &RepoSandbox, mut doc: Value) -> Result<Value, String> {
    let layout = layout();
    let home = path("/Users/minion");
    let input = MacInput {
        layout: &layout,
        login_home: &home,
        developer_dir: DEV,
        capabilities: caps,
        sandbox,
    };
    apply(&mut doc, &input).map(|()| doc)
}

/// The string array at `keys` in `doc`.
fn arr<'a>(doc: &'a Value, keys: &[&str]) -> &'a Vec<Value> {
    let mut v = doc;
    for k in keys {
        v = v.get(*k).unwrap();
    }
    v.as_array().unwrap()
}

fn minimal() -> Value {
    json!({"filesystem": {"read": [], "allow": []}})
}

fn caps(xcode: bool, ios_simulator: bool) -> Vec<MacCapability> {
    selected(Selection {
        xcode,
        ios_simulator,
    })
}

fn pretty(c: &[MacCapability]) -> String {
    let doc = run(c, &RepoSandbox::default(), minimal()).unwrap();
    serde_json::to_string_pretty(&doc).unwrap()
}

#[test]
fn toolchain_snapshot() {
    let expected = r#"{
  "filesystem": {
    "read": [
      "/Applications/Xcode.app/Contents/Developer",
      "/Library/Developer",
      "/Library/Apple",
      "/var/select",
      "/Users/Shared/moor/t/launch"
    ],
    "allow": [
      "/Users/Shared/moor/t/work",
      "/Users/Shared/moor/t/home",
      "/Users/Shared/moor/t/tmp",
      "/Users/Shared/moor/t/cache"
    ]
  }
}"#;
    assert_eq!(pretty(&caps(false, false)), expected);
}

#[test]
fn xcode_snapshot() {
    let expected = r#"{
  "filesystem": {
    "read": [
      "/Applications/Xcode.app/Contents/Developer",
      "/Library/Developer",
      "/Library/Apple",
      "/var/select",
      "/Users/Shared/moor/t/launch",
      "/Users/minion/Library/Developer/CoreSimulator",
      "/Users/minion/Library/Developer/DVTDownloads"
    ],
    "allow": [
      "/Users/Shared/moor/t/work",
      "/Users/Shared/moor/t/home",
      "/Users/Shared/moor/t/tmp",
      "/Users/Shared/moor/t/cache",
      "/Users/Shared/moor/t/home/Library/Developer/Xcode",
      "/Users/Shared/moor/t/home/Library/Caches"
    ]
  },
  "unsafe_macos_seatbelt_rules": [
    "(allow user-preference-read user-preference-write (preference-domain \"com.apple.dt.Xcode\"))",
    "(allow user-preference-read user-preference-write (preference-domain \"com.apple.dt.xcodebuild\"))",
    "(allow user-preference-read user-preference-write (preference-domain \"com.apple.ibtool\"))"
  ]
}"#;
    assert_eq!(pretty(&caps(true, false)), expected);
}

#[test]
fn ios_simulator_snapshot() {
    let expected = r#"{
  "filesystem": {
    "read": [
      "/Applications/Xcode.app/Contents/Developer",
      "/Library/Developer",
      "/Library/Apple",
      "/var/select",
      "/Users/Shared/moor/t/launch",
      "/Users/minion/Library/Developer/CoreSimulator",
      "/Users/minion/Library/Developer/DVTDownloads",
      "/Library/Developer/CoreSimulator"
    ],
    "allow": [
      "/Users/Shared/moor/t/work",
      "/Users/Shared/moor/t/home",
      "/Users/Shared/moor/t/tmp",
      "/Users/Shared/moor/t/cache",
      "/Users/Shared/moor/t/home/Library/Developer/Xcode",
      "/Users/Shared/moor/t/home/Library/Caches",
      "/Users/Shared/moor/t/sim"
    ]
  },
  "unsafe_macos_seatbelt_rules": [
    "(allow user-preference-read user-preference-write (preference-domain \"com.apple.dt.Xcode\"))",
    "(allow user-preference-read user-preference-write (preference-domain \"com.apple.dt.xcodebuild\"))",
    "(allow user-preference-read user-preference-write (preference-domain \"com.apple.ibtool\"))"
  ]
}"#;
    assert_eq!(pretty(&caps(true, true)), expected);
}

#[test]
fn toolchain_emits_no_rules_key() {
    let doc = run(&caps(false, false), &RepoSandbox::default(), minimal()).unwrap();
    assert!(doc.get("unsafe_macos_seatbelt_rules").is_none());
}

#[test]
fn xcode_emits_three_pref_rules() {
    let doc = run(&caps(true, false), &RepoSandbox::default(), minimal()).unwrap();
    let rules = arr(&doc, &["unsafe_macos_seatbelt_rules"]);
    assert_eq!(rules.len(), 3);
    assert!(rules
        .iter()
        .all(|r| r.as_str().unwrap().contains("preference-domain")));
}

#[test]
fn project_path_under_work_is_accepted() {
    let sb = RepoSandbox {
        read: vec!["ios/Pods".to_owned()],
        write: vec!["build".to_owned()],
    };
    let doc = run(&caps(false, false), &sb, minimal()).unwrap();
    let read = arr(&doc, &["filesystem", "read"]);
    let allow = arr(&doc, &["filesystem", "allow"]);
    assert!(read.contains(&json!("/Users/Shared/moor/t/work/ios/Pods")));
    assert!(allow.contains(&json!("/Users/Shared/moor/t/work/build")));
}

#[test]
fn project_path_outside_work_is_refused() {
    let sb = RepoSandbox {
        read: vec!["../x".to_owned()],
        write: vec![],
    };
    let err = run(&caps(false, false), &sb, minimal()).unwrap_err();
    assert!(err.contains("box.sandbox.read[0]"), "{err}");
    assert!(err.contains("outside the workdir"), "{err}");
    let sb = RepoSandbox {
        read: vec![],
        write: vec!["ok".to_owned(), "/etc".to_owned()],
    };
    let err = run(&caps(false, false), &sb, minimal()).unwrap_err();
    assert!(err.contains("box.sandbox.write[1]"), "{err}");
}

#[test]
fn layout_inside_login_home_is_refused() {
    let key = TaskKey::new(&TaskName::parse("t").unwrap());
    let layout = MacLayout::new(&path("/Users/minion/moor"), &key).unwrap();
    let home = path("/Users/minion");
    let sb = RepoSandbox::default();
    let c = caps(false, false);
    let input = MacInput {
        layout: &layout,
        login_home: &home,
        developer_dir: DEV,
        capabilities: &c,
        sandbox: &sb,
    };
    let err = apply(&mut minimal(), &input).unwrap_err();
    assert!(err.contains("inside the login home"), "{err}");
}

#[test]
fn no_output_path_is_library_logs() {
    for c in [caps(false, false), caps(true, false), caps(true, true)] {
        assert!(!pretty(&c).contains("Library/Logs"));
    }
}

#[test]
fn duplicates_are_not_re_added() {
    let c = caps(true, true);
    let once = run(&c, &RepoSandbox::default(), minimal()).unwrap();
    let twice = run(&c, &RepoSandbox::default(), once.clone()).unwrap();
    assert_eq!(once, twice);
}

#[test]
fn missing_arrays_are_created() {
    let doc = run(&caps(false, false), &RepoSandbox::default(), json!({})).unwrap();
    assert_eq!(arr(&doc, &["filesystem", "allow"]).len(), 4);
}
