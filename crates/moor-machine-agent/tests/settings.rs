//! The checkpoint setting in crabbox's user config: added when missing, never changed once set.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_docs_in_private_items,
    reason = "tests"
)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Command;

use moor_machine_agent::settings::{ensure, opted_out, Ensured};
use serde_json::Value;

const ON: &str =
    "external:\n  capabilities:\n    checkpoints: true\n  config:\n    checkpoints: true\n";

fn value(path: &Path) -> Value {
    serde_saphyr::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn checkpoints(path: &Path) -> Option<Value> {
    value(path)
        .pointer("/external/capabilities/checkpoints")
        .cloned()
}

fn adapter_gate(path: &Path) -> Option<Value> {
    value(path).pointer("/external/config/checkpoints").cloned()
}

fn config() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    (dir, path)
}

#[test]
fn a_missing_file_gets_only_the_key_and_is_private() {
    let (_dir, path) = config();
    assert_eq!(ensure(&path, "external").unwrap(), Ensured::Added);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), ON);
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn each_shape_of_a_config_gets_the_key_and_keeps_the_rest_of_it_word_for_word() {
    let cases = [
        ("", ON.to_owned()),
        ("# mine\n", format!("# mine\n{ON}")),
        ("provider: aws # keep\n", format!("provider: aws # keep\n{ON}")),
        (
            "external:\n  command: x # keep\nother: 1\n",
            format!("{ON}  command: x # keep\nother: 1\n"),
        ),
        (
            "external:\n    command: x\n",
            "external:\n    capabilities:\n      checkpoints: true\n    \
             config:\n      checkpoints: true\n    command: x\n"
                .to_owned(),
        ),
        (
            "external:\n  capabilities:\n    cacheVolumes: true # keep\n  command: x\n",
            "external:\n  config:\n    checkpoints: true\n  capabilities:\n    checkpoints: true\n    cacheVolumes: true # keep\n  command: x\n"
                .to_owned(),
        ),
        (
            "external:\n  capabilities: { cacheVolumes: true } # keep\n  config: { bootTemplate: true } # mine\n",
            "external:\n  capabilities: { checkpoints: true, cacheVolumes: true } # keep\n  config: { checkpoints: true, bootTemplate: true } # mine\n"
                .to_owned(),
        ),
        (
            "external:\n  config:\n    bootTemplate: false # keep\n",
            "external:\n  capabilities:\n    checkpoints: true\n  config:\n    checkpoints: true\n    bootTemplate: false # keep\n"
                .to_owned(),
        ),
        ("external:\n", ON.to_owned()),
    ];
    for (before, after) in cases {
        let (_dir, path) = config();
        std::fs::write(&path, before).unwrap();
        assert_eq!(
            ensure(&path, "external").unwrap(),
            Ensured::Added,
            "{before}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), after, "{before}");
        assert_eq!(
            ensure(&path, "external").unwrap(),
            Ensured::Kept,
            "{before}"
        );
    }
}

#[test]
fn a_shape_no_line_edit_fits_still_means_the_same_plus_the_key() {
    let (_dir, path) = config();
    std::fs::write(&path, "external: {command: x}\nother: 1\n").unwrap();
    assert_eq!(ensure(&path, "external").unwrap(), Ensured::Added);
    assert_eq!(checkpoints(&path), Some(Value::Bool(true)));
    assert_eq!(adapter_gate(&path), Some(Value::Bool(true)));
    let got = value(&path);
    assert_eq!(got.pointer("/external/command"), Some(&Value::from("x")));
    assert_eq!(got.pointer("/other"), Some(&Value::from(1)));
}

#[test]
fn a_value_already_set_stays_and_the_file_is_not_touched() {
    for text in [
        "external:\n  capabilities:\n    checkpoints: false # no thanks\n  config:\n    checkpoints: false\n",
        "external:\n  capabilities: { checkpoints: false }\n  config: { checkpoints: true }\n",
        "external: {capabilities: {checkpoints: true}, config: {checkpoints: false}}\n",
    ] {
        let (_dir, path) = config();
        std::fs::write(&path, text).unwrap();
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(ensure(&path, "external").unwrap(), Ensured::Kept, "{text}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );
    }
}

#[test]
fn only_the_external_provider_has_the_setting() {
    let (_dir, path) = config();
    assert_eq!(ensure(&path, "aws").unwrap(), Ensured::Kept);
    assert!(!path.exists());
}

#[test]
fn a_file_it_cannot_read_as_a_config_is_left_alone() {
    for text in [
        "{not yaml",
        "- a\n- list\n",
        "external: 3\n",
        "external:\n  capabilities: 3\n",
        "external:\n  config: 3\n",
    ] {
        let (_dir, path) = config();
        std::fs::write(&path, text).unwrap();
        assert!(ensure(&path, "external").is_err(), "{text}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
}

#[test]
fn a_write_keeps_the_mode_and_a_symlink_and_leaves_no_stray_file() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.yaml");
    let link = dir.path().join("config.yaml");
    std::fs::write(&real, "other: 1\n").unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(ensure(&link, "external").unwrap(), Ensured::Added);
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(checkpoints(&real), Some(Value::Bool(true)));
    let mode = std::fs::metadata(&real).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o640);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn an_owners_false_on_one_key_stays_and_the_other_is_still_added() {
    for (text, kept, added) in [
        (
            "external:\n  capabilities:\n    checkpoints: false\n",
            "/external/capabilities/checkpoints",
            "/external/config/checkpoints",
        ),
        (
            "external:\n  config:\n    checkpoints: false\n",
            "/external/config/checkpoints",
            "/external/capabilities/checkpoints",
        ),
    ] {
        let (_dir, path) = config();
        std::fs::write(&path, text).unwrap();
        assert_eq!(ensure(&path, "external").unwrap(), Ensured::Added);
        let got = value(&path);
        assert_eq!(got.pointer(kept), Some(&Value::Bool(false)), "{text}");
        assert_eq!(got.pointer(added), Some(&Value::Bool(true)), "{text}");
    }
}

#[test]
fn a_host_with_only_the_capability_gets_the_adapters_key_on_its_next_run() {
    let (_dir, path) = config();
    let text = "external:\n  capabilities:\n    checkpoints: true # mine\n";
    std::fs::write(&path, text).unwrap();
    assert_eq!(ensure(&path, "external").unwrap(), Ensured::Added);
    assert_eq!(adapter_gate(&path), Some(Value::Bool(true)));
    assert!(std::fs::read_to_string(&path).unwrap().contains("# mine"));
}

#[test]
fn only_an_owners_false_counts_as_opting_out_and_it_names_the_key() {
    let (_dir, path) = config();
    assert_eq!(opted_out(&path, "external"), None);
    std::fs::write(&path, "other: 1\n").unwrap();
    assert_eq!(opted_out(&path, "external"), None);
    std::fs::write(&path, ON).unwrap();
    assert_eq!(opted_out(&path, "external"), None);
    let cap = "external:\n  capabilities:\n    checkpoints: false\n";
    std::fs::write(&path, cap).unwrap();
    assert_eq!(
        opted_out(&path, "external"),
        Some("capabilities.checkpoints")
    );
    assert_eq!(opted_out(&path, "aws"), None);
    let gate = "external:\n  config:\n    checkpoints: false\n";
    std::fs::write(&path, gate).unwrap();
    assert_eq!(opted_out(&path, "external"), Some("config.checkpoints"));
}

/// Runs the agent with `args` under `home`, its config in crabbox's default place.
fn agent(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_moor-machine-agent"))
        .args(args)
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("CRABBOX_CONFIG")
        .output()
        .unwrap()
}

fn default_config(home: &Path) -> std::path::PathBuf {
    let dir = if cfg!(target_os = "macos") {
        "Library/Application Support"
    } else {
        ".config"
    };
    home.join(dir).join("crabbox/config.yaml")
}

const ENROLL: [&str; 6] = [
    "enroll",
    "--server",
    "http://127.0.0.1:1",
    "--name",
    "n",
    "--paste",
];

#[test]
fn enroll_for_the_external_provider_writes_the_setting_and_says_so() {
    let home = tempfile::tempdir().unwrap();
    let mut args = ENROLL.to_vec();
    args.extend(["--runtime", "external"]);
    let out = agent(home.path(), &args);
    assert_eq!(
        checkpoints(&default_config(home.path())),
        Some(Value::Bool(true))
    );
    let said = String::from_utf8(out.stderr).unwrap();
    assert!(said.contains("config.yaml"), "{said}");
}

#[test]
fn the_directories_it_makes_are_the_users_alone() {
    let home = tempfile::tempdir().unwrap();
    agent(home.path(), &["run", "--runtime", "external"]);
    let config = default_config(home.path());
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let crabbox = config.parent().unwrap();
    assert_eq!(mode(crabbox), 0o700);
    assert_eq!(mode(crabbox.parent().unwrap()), 0o700);
}

#[test]
fn enroll_without_a_runtime_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    agent(home.path(), &ENROLL);
    assert!(!default_config(home.path()).exists());
}

#[test]
fn every_run_writes_the_setting_for_a_host_enrolled_before_it_existed() {
    let home = tempfile::tempdir().unwrap();
    let out = agent(home.path(), &["run", "--runtime", "external"]);
    assert!(!out.status.success());
    assert_eq!(
        checkpoints(&default_config(home.path())),
        Some(Value::Bool(true))
    );
}

#[test]
fn a_run_keeps_an_owners_false() {
    let home = tempfile::tempdir().unwrap();
    let path = default_config(home.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text =
        "external:\n  capabilities:\n    checkpoints: false\n  config:\n    checkpoints: false\n";
    std::fs::write(&path, text).unwrap();
    agent(home.path(), &["run", "--runtime", "external"]);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn a_run_with_an_unreadable_config_warns_and_goes_on() {
    let home = tempfile::tempdir().unwrap();
    let path = default_config(home.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{not yaml").unwrap();
    let out = agent(home.path(), &["run", "--runtime", "external"]);
    let said = String::from_utf8(out.stderr).unwrap();
    assert!(said.contains("cannot parse"), "{said}");
    assert!(said.contains("enroll first"), "{said}");
}
