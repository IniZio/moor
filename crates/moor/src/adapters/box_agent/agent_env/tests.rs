use std::process::Command;

use std::collections::BTreeMap;

use moor_core::name::BoxPath;

use super::{clear_lines, export_lines, profile_lines, ROOT_SANDBOX_VAR};

fn pair(n: &str, v: &str) -> (String, String) {
    (n.to_owned(), v.to_owned())
}

fn sh(script: &str, env: &[(&str, &str)]) -> String {
    let out = Command::new("sh")
        .arg("-c")
        .arg(script)
        .env_clear()
        .envs(env.iter().copied())
        .output()
        .expect("sh runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8")
}

#[test]
fn clear_lines_are_two_lines_naming_what_they_clear() {
    let lines = clear_lines();
    let [gate, loop_line] = lines.as_slice() else {
        unreachable!("{lines:?}");
    };
    assert_eq!(
        gate,
        "unset child child_identity child_pid command_umask gate run_dir state token"
    );
    assert!(loop_line.contains("NSC_"));
}

#[test]
fn clear_lines_drop_crabbox_names_and_keep_the_rest() {
    let script = format!(
        "PATH=/usr/bin:/bin; export PATH; {}; env",
        clear_lines().join("; ")
    );
    let out = sh(
        &script,
        &[
            ("PATH", "/usr/bin:/bin"),
            ("child", "1"),
            ("child_identity", "1"),
            ("child_pid", "1"),
            ("command_umask", "1"),
            ("gate", "1"),
            ("run_dir", "1"),
            ("state", "1"),
            ("NSC_X", "1"),
            ("TOKEN", "1"),
            ("Token", "1"),
            ("token", "1"),
            ("KEEP_ME", "1"),
            ("GH_TOKEN", "1"),
        ],
    );
    let names: Vec<&str> = out.lines().filter_map(|l| l.split('=').next()).collect();
    for gone in [
        "child",
        "child_identity",
        "child_pid",
        "command_umask",
        "gate",
        "run_dir",
        "state",
        "NSC_X",
        "TOKEN",
        "Token",
        "token",
    ] {
        assert!(!names.contains(&gone), "{gone} survived:\n{out}");
    }
    assert!(names.contains(&"KEEP_ME"));
    assert!(names.contains(&"GH_TOKEN"));
}

#[test]
fn export_lines_add_the_root_marker_last_only_for_root() {
    let vars = [pair("A", "1")];
    assert_eq!(
        export_lines(&vars, true, &[]).unwrap(),
        ["export A='1'", "export IS_SANDBOX=1"]
    );
    assert_eq!(export_lines(&vars, false, &[]).unwrap(), ["export A='1'"]);
}

#[test]
fn export_lines_skip_stripped_names() {
    let vars = [pair("A", "1"), pair("B", "2")];
    let lines = export_lines(&vars, false, &["A".to_owned()]).unwrap();
    assert_eq!(lines, ["export B='2'"]);
}

#[test]
fn a_repo_value_cannot_replace_the_root_marker() {
    let vars = [pair(ROOT_SANDBOX_VAR, "0")];
    assert_eq!(
        export_lines(&vars, true, &[]).unwrap(),
        ["export IS_SANDBOX=1"]
    );
    assert!(export_lines(&vars, false, &[]).unwrap().is_empty());
}

#[test]
fn a_bad_name_is_an_error() {
    for bad in ["", "1A", "A-B", "A B", "A=B", "A;rm"] {
        let err = export_lines(&[pair(bad, "v")], false, &[]).unwrap_err();
        assert!(err.contains(bad), "{err}");
    }
}

#[test]
fn quoted_values_round_trip_through_sh() {
    let value = "it's \"a\" $HOME `x` \\ ; \n end";
    let mut script = export_lines(&[pair("V", value)], false, &[])
        .unwrap()
        .join("\n");
    script.push_str("\nprintf %s \"$V\"");
    assert_eq!(sh(&script, &[]), value);
}

fn profile_env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(n, v)| pair(n, v)).collect()
}

fn dirs() -> (BoxPath, BoxPath) {
    let home = BoxPath::restore("/home/box").unwrap();
    let profile = BoxPath::restore("/home/box/.moor-profile/person").unwrap();
    (home, profile)
}

#[test]
#[expect(
    clippy::literal_string_with_formatting_args,
    reason = "the profile's placeholders"
)]
fn profile_lines_fill_the_home_and_profile_placeholders() {
    let (home, profile) = dirs();
    let env = profile_env(&[
        ("BUN_OPTIONS", "--preload={profile}/claude/x.js"),
        ("H", "{home}/b"),
    ]);
    let lines = profile_lines(&env, (&home, &profile), &[]).unwrap();
    assert_eq!(
        lines,
        [
            "export BUN_OPTIONS='--preload=/home/box/.moor-profile/person/claude/x.js'",
            "export H='/home/box/b'",
        ]
    );
}

#[test]
#[expect(
    clippy::literal_string_with_formatting_args,
    reason = "the profile's placeholders"
)]
fn a_profile_value_stays_literal_under_sh() {
    let dir = tempfile::tempdir().unwrap();
    let (home, profile) = dirs();
    let env = profile_env(&[("V", "$(touch x) `touch y` $HOME {home}")]);
    let mut script = profile_lines(&env, (&home, &profile), &[])
        .unwrap()
        .join("\n");
    script.push_str("\nprintf %s \"$V\"");
    let out = Command::new("sh")
        .arg("-c")
        .arg(script)
        .current_dir(dir.path())
        .env_clear()
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "$(touch x) `touch y` $HOME /home/box"
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn profile_lines_skip_stripped_names_and_refuse_bad_ones() {
    let (home, profile) = dirs();
    let env = profile_env(&[("A", "1"), ("B", "2")]);
    let lines = profile_lines(&env, (&home, &profile), &["A".to_owned()]).unwrap();
    assert_eq!(lines, ["export B='2'"]);
    let bad = profile_env(&[("A;rm", "1")]);
    assert!(profile_lines(&bad, (&home, &profile), &[]).is_err());
}
