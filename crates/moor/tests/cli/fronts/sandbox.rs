//! A front may write its own state dir and nothing else.

use std::fs;
use std::path::Path;

use serde_json::Value;

use super::{wait_for, Controller, PROBE};

#[test]
fn a_front_writes_its_own_state_dir_and_nothing_else() {
    let ctl = Controller::start(PROBE, &[]);
    let got = wait_for("the probe", || ctl.line("probe "));
    let moor_dir = Path::new(env!("CARGO_BIN_EXE_moor")).parent().unwrap();
    let planted = moor_dir.join(format!(
        "planted-{}",
        ctl.root.file_name().unwrap().to_string_lossy()
    ));
    let _ = fs::remove_file(planted);
    let got: Value = serde_json::from_str(&got).unwrap();
    assert_eq!(got["own"], "answers", "{got}");
    assert_eq!(got["own_dir"], "writable", "{got}");
    assert!(ctl.root.join("front").join("planted").exists());
    assert_eq!(got["tmp"], "writable", "{got}");
    assert!(ctl.root.join("front").join("tmp").join("planted").exists());
    assert_eq!(got["dev_null"], "writable", "{got}");
    for denied in ["config", "home", "root", "moor_dir"] {
        assert_eq!(got[denied], "Permission denied", "{denied}: {got}");
    }
}
