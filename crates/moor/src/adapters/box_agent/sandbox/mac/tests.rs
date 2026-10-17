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
        toolchains: &[],
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
        swiftpm_plugins: false,
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
    ],
    "deny": [
      "/Users/minion/.config/gh",
      "/Users/minion/.docker",
      "/Users/minion/.ssh"
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
    ],
    "deny": [
      "/Users/minion/.config/gh",
      "/Users/minion/.docker",
      "/Users/minion/.ssh"
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
    ],
    "deny": [
      "/Users/minion/.config/gh",
      "/Users/minion/.docker",
      "/Users/minion/.ssh"
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
        toolchains: &[],
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

const NESTED: &str = r#"(allow process-exec (with no-sandbox) (literal "/usr/bin/sandbox-exec"))"#;

fn rules_of(doc: &Value) -> Vec<&str> {
    doc.get("unsafe_macos_seatbelt_rules")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

#[test]
fn swiftpm_plugins_renders_the_nested_sandbox_rule_once() {
    let c = selected(Selection {
        swiftpm_plugins: true,
        ..Selection::default()
    });
    let doc = run(&c, &RepoSandbox::default(), minimal()).unwrap();
    assert_eq!(rules_of(&doc).iter().filter(|r| **r == NESTED).count(), 1);
    let twice = run(&c, &RepoSandbox::default(), doc.clone()).unwrap();
    assert_eq!(doc, twice);
}

#[test]
fn nested_sandbox_rule_is_absent_without_the_capability() {
    for c in [caps(false, false), caps(true, false), caps(true, true)] {
        assert!(!pretty(&c).contains("sandbox-exec"));
    }
}

const CREDS: [&str; 3] = [
    "/Users/minion/.config/gh",
    "/Users/minion/.docker",
    "/Users/minion/.ssh",
];

fn covers(grant: &str, cred: &str) -> bool {
    let (g, c) = (grant.trim_end_matches('/'), cred);
    c == g || c.starts_with(&format!("{g}/"))
}

#[test]
fn login_home_credentials_are_denied() {
    let doc = run(&caps(true, true), &RepoSandbox::default(), minimal()).unwrap();
    let deny = arr(&doc, &["filesystem", "deny"]);
    for c in CREDS {
        assert!(deny.contains(&json!(c)), "{c}");
    }
}

#[test]
fn no_grant_covers_a_credential_directory() {
    let sb = RepoSandbox {
        read: vec!["ios".to_owned()],
        write: vec!["build".to_owned()],
    };
    let doc = run(&caps(true, true), &sb, minimal()).unwrap();
    for key in ["read", "allow", "read_file", "allow_file"] {
        let Some(list) = doc
            .get("filesystem")
            .and_then(|f| f.get(key))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for g in list.iter().filter_map(Value::as_str) {
            for c in CREDS {
                assert!(!covers(g, c), "{key} `{g}` covers {c}");
            }
        }
    }
}

#[test]
fn task_home_keeps_its_gh_config_grant() {
    let doc = run(&caps(false, false), &RepoSandbox::default(), minimal()).unwrap();
    let allow = arr(&doc, &["filesystem", "allow"]);
    assert!(allow.contains(&json!("/Users/Shared/moor/t/home")));
    assert!(!arr(&doc, &["filesystem", "deny"])
        .iter()
        .any(|d| d.as_str().unwrap().starts_with("/Users/Shared")));
}

#[test]
fn project_read_of_login_home_config_is_refused() {
    let sb = RepoSandbox {
        read: vec!["/Users/minion/.config".to_owned()],
        write: vec![],
    };
    let err = run(&caps(false, false), &sb, minimal()).unwrap_err();
    assert!(err.contains("box.sandbox.read[0]"), "{err}");
}

#[test]
fn a_grant_covering_a_credential_directory_is_refused() {
    let denies = vec!["/Users/minion/.config/gh".to_owned()];
    for grant in [
        "/Users/minion",
        "/Users/minion/.config",
        "/Users/minion/.config/gh/x",
    ] {
        let g = grant.to_owned();
        let err = super::refuse_credentials(&denies, [&g].into_iter()).unwrap_err();
        assert!(err.contains("holds login credentials"), "{err}");
    }
    let ok = "/Users/minion/.config/other".to_owned();
    assert!(super::refuse_credentials(&denies, [&ok].into_iter()).is_ok());
}

#[test]
fn apply_refuses_a_grant_that_reaches_the_login_credentials() {
    // The Task home is above the login home, so the home grant is not inside the login home but
    // contains its credential directories.
    let layout = layout();
    let home = path("/Users/Shared/moor/t/home/login");
    let sb = RepoSandbox::default();
    let c = caps(false, false);
    let input = MacInput {
        layout: &layout,
        login_home: &home,
        developer_dir: DEV,
        capabilities: &c,
        sandbox: &sb,
        toolchains: &[],
    };
    let err = apply(&mut minimal(), &input).unwrap_err();
    assert!(err.contains("holds login credentials"), "{err}");
    assert!(err.contains("/Users/Shared/moor/t/home"), "{err}");
}

fn with_toolchain(root: &str) -> Result<Value, String> {
    let layout = layout();
    let home = path("/Users/minion");
    let sb = RepoSandbox::default();
    let c = caps(false, false);
    let tc = [("mise".to_owned(), root.to_owned())];
    let input = MacInput {
        layout: &layout,
        login_home: &home,
        developer_dir: DEV,
        capabilities: &c,
        sandbox: &sb,
        toolchains: &tc,
    };
    let mut doc = minimal();
    apply(&mut doc, &input).map(|()| doc)
}

#[test]
fn host_toolchain_root_is_read_only() {
    let doc = with_toolchain("/Users/minion/.local/share/mise").unwrap();
    let root = json!("/Users/minion/.local/share/mise");
    assert!(arr(&doc, &["filesystem", "read"]).contains(&root));
    assert!(!arr(&doc, &["filesystem", "allow"]).contains(&root));
}

#[test]
fn host_toolchain_roots_that_widen_access_are_refused() {
    for (root, why) in [
        (
            "/Users/minion/.ssh",
            "inside the credential directory /Users/minion/.ssh",
        ),
        (
            "/Users/minion/.config/gh/x",
            "inside the credential directory",
        ),
        ("/Users/minion/.config", "contains the credential directory"),
        ("/Users/minion", "login home"),
        ("/Users", "login home"),
        ("/", "login home"),
        ("/Users/minion/Library/x", "inside /Users/minion/Library"),
        ("/Users/Shared/moor/other/work", "Box work root"),
        ("/Users/Shared", "Box work root"),
        ("/Users/minion/../x", "plain path"),
        ("/tmp/a\"b", "plain path"),
    ] {
        let err = with_toolchain(root).unwrap_err();
        assert!(
            err.starts_with(&format!("host toolchain \"mise\" root {root} is refused")),
            "{err}"
        );
        assert!(err.contains(why), "{root}: {err}");
    }
}
