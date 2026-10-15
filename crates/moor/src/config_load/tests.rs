//! Config loading tests.

use std::path::PathBuf;

use moor_core::config::UserConfig;
use moor_core::ErrorKind;

use super::{load_repo, load_user, private_paths, user_path};
use crate::env::Env;

type Case<'a> = (&'a [(&'a str, &'a str)], Option<&'a str>);

fn env(pairs: &[(&str, &str)]) -> Env {
    Env::from_pairs(pairs.iter().copied())
}

#[test]
fn user_path_precedence() {
    let cases: [Case; 6] = [
        (
            &[
                ("MOOR_CONFIG", "/m.yaml"),
                ("XDG_CONFIG_HOME", "/x"),
                ("HOME", "/h"),
            ],
            Some("/m.yaml"),
        ),
        (
            &[("XDG_CONFIG_HOME", "/x"), ("HOME", "/h")],
            Some("/x/moor/config.yaml"),
        ),
        (&[("HOME", "/h")], Some("/h/.config/moor/config.yaml")),
        (
            &[("MOOR_CONFIG", ""), ("XDG_CONFIG_HOME", ""), ("HOME", "/h")],
            Some("/h/.config/moor/config.yaml"),
        ),
        (&[], None),
        (&[("MOOR_CONFIG", ""), ("XDG_CONFIG_HOME", "")], None),
    ];
    for (pairs, want) in cases {
        assert_eq!(user_path(&env(pairs)), want.map(PathBuf::from), "{pairs:?}");
    }
}

#[test]
fn no_config_location_means_defaults() {
    assert_eq!(load_user(&Env::default()).unwrap(), UserConfig::default());
    assert_eq!(load_user(&env(&[])).unwrap(), UserConfig::default());
}

#[test]
fn missing_file_is_default() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("nope.yaml");
    let c = load_user(&env(&[("MOOR_CONFIG", p.to_str().unwrap())])).unwrap();
    assert_eq!(c, UserConfig::default());
}

#[test]
fn directory_is_failed_naming_path() {
    let d = tempfile::tempdir().unwrap();
    let e = load_user(&env(&[("MOOR_CONFIG", d.path().to_str().unwrap())])).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(e.error.to_string().contains(d.path().to_str().unwrap()));
}

#[test]
fn bad_yaml_is_failed_with_file_label() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("c.yaml");
    std::fs::write(&p, "bogus_key: 1\n").unwrap();
    let e = load_user(&env(&[("MOOR_CONFIG", p.to_str().unwrap())])).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(e.error.to_string().starts_with(p.to_str().unwrap()));
}

#[test]
fn repo_worktree_first_then_main_then_none() {
    let wt = tempfile::tempdir().unwrap();
    let main = tempfile::tempdir().unwrap();
    assert!(load_repo(wt.path(), main.path()).unwrap().is_none());
    std::fs::write(main.path().join(".moor.yaml"), "{}\n").unwrap();
    let from_main = load_repo(wt.path(), main.path()).unwrap();
    assert!(from_main.is_some());
    std::fs::write(wt.path().join(".moor.yaml"), "bogus_key: 1\n").unwrap();
    let e = load_repo(wt.path(), main.path()).unwrap_err();
    assert!(e.error.to_string().starts_with(wt.path().to_str().unwrap()));
}

#[test]
fn private_paths_are_the_state_dir_and_the_config_dir() {
    let got = private_paths(&env(&[("HOME", "/Users/o")]));
    assert_eq!(got, ["/Users/o/.local/state/moor", "/Users/o/.config/moor"]);
    let got = private_paths(&env(&[
        ("MOOR_STATE_DIR", "/s"),
        ("MOOR_CONFIG", "/c/x.yaml"),
    ]));
    assert_eq!(got, ["/s", "/c"]);
    assert!(private_paths(&env(&[])).is_empty());
}
