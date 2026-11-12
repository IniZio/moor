//! `run` start-up keeps the Boxes slice and the provider's setting in place, for a host whose
//! agent updated itself and never reran `up`. A stand-in `systemctl` records its calls.
#![allow(
    clippy::unwrap_used,
    clippy::missing_docs_in_private_items,
    reason = "tests"
)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

const SYSTEMCTL: &str = r#"#!/bin/sh
echo "$*" >> "$HOME/systemctl.log"
if [ -e "$HOME/no-bus" ]; then echo "Failed to connect to bus: No medium found" >&2; exit 1; fi
"#;

struct Host {
    home: tempfile::TempDir,
}

impl Host {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let systemctl = bin.join("systemctl");
        std::fs::write(&systemctl, SYSTEMCTL).unwrap();
        std::fs::set_permissions(&systemctl, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { home }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.home.path().join(name)
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.path("systemctl.log")).unwrap_or_default()
    }

    /// What `run --runtime external` said before it stopped for want of a pairing.
    fn run(&self, units: &Path) -> String {
        let path = format!(
            "{}:{}",
            self.path("bin").display(),
            std::env::var("PATH").unwrap()
        );
        let out = Command::new(env!("CARGO_BIN_EXE_moor-machine-agent"))
            .args(["run", "--runtime", "external"])
            .env("HOME", self.home.path())
            .env("XDG_STATE_HOME", self.path("state"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("MOOR_AGENT_SERVICE_DIR", units)
            .env("PATH", path)
            .output()
            .unwrap();
        String::from_utf8(out.stderr).unwrap()
    }

    fn config(&self) -> String {
        std::fs::read_to_string(self.path("config/crabbox/config.yaml")).unwrap_or_default()
    }
}

#[test]
fn run_writes_the_slice_and_the_setting_and_a_second_start_changes_nothing() {
    let host = Host::new();
    let units = host.path("units");
    let said = host.run(&units);
    assert!(units.join("moorboxes.slice").exists(), "{said}");
    assert!(
        host.config().contains("cgroupSlice: \"moorboxes.slice\""),
        "{}",
        host.config()
    );
    assert_eq!(host.calls().matches("--user daemon-reload").count(), 1);

    let (config, calls) = (host.config(), host.calls());
    let again = host.run(&units);
    assert_eq!(host.config(), config);
    assert_eq!(host.calls(), calls, "{again}");
}

#[test]
fn a_slice_that_cannot_be_set_up_is_reported_once_and_the_agent_carries_on() {
    for failure in ["no bus", "unwritable"] {
        let host = Host::new();
        let units = host.path("units");
        if failure == "no bus" {
            std::fs::write(host.path("no-bus"), "").unwrap();
        } else {
            std::fs::write(&units, "a file, not a directory").unwrap();
        }
        let said = host.run(&units);
        let reports = said.matches("no memory ceiling").count();
        assert_eq!(reports, 1, "{failure}: {said}");
        assert!(said.contains("run moor-machine-agent up first"), "{said}");
    }
}
