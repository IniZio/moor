#![allow(clippy::unwrap_used, clippy::panic, reason = "test assertions")]

use crate::config::{parse_repo, parse_user, sync_excludes, SyncConfig};

fn repo(sync: &str) -> SyncConfig {
    let text = format!("machine:\n  labels: [macos]\n  sync: {sync}\n");
    parse_repo("m.yaml", &text).unwrap().machine.unwrap().sync
}

fn user(sync: &str) -> SyncConfig {
    let text = format!("machines:\n  sync: {sync}\n");
    parse_user("u.yaml", &text).unwrap().machines.sync
}

const ALL: [&str; 7] = [
    "node_modules",
    "Pods",
    ".gradle",
    "DerivedData",
    "*.xcworkspace/xcuserdata",
    ".dart_tool",
    "target",
];

#[test]
fn the_default_leaves_every_preset_alone() {
    let none = SyncConfig::default();
    assert_eq!(sync_excludes(&none, &none), ALL);
    assert_eq!(sync_excludes(&repo("{}"), &user("{}")), ALL);
}

#[test]
fn presets_false_gives_none() {
    let off = repo("{presets: false}");
    assert!(sync_excludes(&off, &SyncConfig::default()).is_empty());
    assert!(sync_excludes(&SyncConfig::default(), &user("{presets: false}")).is_empty());
}

#[test]
fn a_dropped_preset_is_the_only_one_gone() {
    let got = sync_excludes(&repo("{presets: [node]}"), &SyncConfig::default());
    assert_eq!(got, &ALL[1..]);
}

#[test]
fn either_layer_drops_a_preset_and_both_add_patterns() {
    let r = repo("{presets: [node], exclude: [app/build, Pods]}");
    let u = user("{presets: [rust], exclude: [ios/tmp]}");
    let got = sync_excludes(&r, &u);
    let want = [
        "Pods",
        ".gradle",
        "DerivedData",
        "*.xcworkspace/xcuserdata",
        ".dart_tool",
        "app/build",
        "ios/tmp",
    ];
    assert_eq!(got, want);
}

#[test]
fn a_bad_entry_is_refused_naming_its_key() {
    let cases = [
        ("{exclude: ['']}", "exclude[0]"),
        ("{exclude: [ok, '-rf']}", "exclude[1]"),
        ("{exclude: [\"a\\nb\"]}", "exclude[0]"),
        ("{presets: [nodejs]}", "presets[0]"),
    ];
    for (sync, key) in cases {
        let text = format!("machine:\n  labels: [macos]\n  sync: {sync}\n");
        let e = parse_repo("m.yaml", &text).unwrap_err();
        assert_eq!(e.path, format!("machine.sync.{key}"), "{sync}: {e}");
        let e = parse_user("u.yaml", &format!("machines:\n  sync: {sync}\n")).unwrap_err();
        assert_eq!(e.path, format!("machines.sync.{key}"), "{sync}: {e}");
    }
    let many: Vec<String> = (0..65).map(|i| format!("p{i}")).collect();
    let text = format!("machines:\n  sync: {{exclude: [{}]}}\n", many.join(", "));
    let e = parse_user("u.yaml", &text).unwrap_err();
    assert_eq!(e.path, "machines.sync.exclude", "{e}");
    assert!(e.to_string().contains("at most 64"), "{e}");
}
