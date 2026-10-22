//! A Box reached through a fake ssh that runs the remote command on this host.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::name::BoxHandle;
use tempfile::TempDir;

use crate::adapters::crabbox::Crabbox;
use crate::adapters::proc::{SystemProc, ToolPaths};
use crate::env::Env;
use crate::testing::clock::FakeClock;
use crate::testing::rng::FakeRng;

/// Hands an `ssh -G` read to the real ssh, recording its argv in `opts.<n>`. Records any other
/// argv NUL-separated in `ssh.<n>`, then runs its last word, the remote command, under sh with
/// stdin passed through. A `drop` file makes it lose the connection: it exits 255
/// and prints nothing, as a call does whose shared master lost the network. A `cut` file holding
/// a byte count makes it lose the connection after that much stdout.
const FAKE_SSH: &str = r#"#!/bin/sh
d=$(dirname "$0")
if test "$1" = -G; then
  printf '%s\0' "$@" > "$d/opts.$(ls "$d" | grep -c '^opts\.')"
  exec /usr/bin/ssh "$@"
fi
n=$(ls "$d" | grep -c '^ssh\.')
printf '%s\0' "$@" > "$d/ssh.$n"
if test -f "$d/drop"; then cat >/dev/null; exit 255; fi
for last; do :; done
if test -f "$d/cut"; then
  TMPDIR="$d/boxtmp" sh -c "$last" | head -c "$(cat "$d/cut")"
  exit 255
fi
TMPDIR="$d/boxtmp" exec sh -c "$last"
"#;

/// Records its argv in `crabbox.<n>`; answers `ssh` with the words crabbox prints for a pinned
/// lease, or with the contents of a `words` file, and fails anything else. `ssh --help` is
/// recorded in `help.<n>` instead and lists `-no-probe` only when a `no-probe` file exists, as
/// the IniZio/crabbox builds with that flag do, and exits 2 when a `help-fails` file exists.
const FAKE_CRABBOX: &str = r#"#!/bin/sh
d=$(dirname "$0")
if test "$1 $2" = "ssh --help"; then
  touch "$d/help.$(ls "$d" | grep -c '^help\.')"
  echo "Usage of ssh:" >&2
  echo "  -id string" >&2
  if test -f "$d/no-probe"; then echo "  -no-probe" >&2; fi
  if test -f "$d/help-fails"; then exit 2; fi
  exit 0
fi
n=$(ls "$d" | grep -c '^crabbox\.')
printf '%s\0' "$@" > "$d/crabbox.$n"
test "$1" = ssh || exit 99
if test -f "$d/words"; then exec cat "$d/words"; fi
echo "ssh -o 'ControlMaster=no' -o 'ControlPath=none' -o 'StrictHostKeyChecking=yes' -o 'UserKnownHostsFile=/lease/known_hosts' -o 'GlobalKnownHostsFile=none' -o 'HostKeyAlias=crabbox-lease-1f2e' 'u@box'"
"#;

/// The host-key words every ssh call must carry.
const PINNED: [&str; 4] = [
    "StrictHostKeyChecking=yes",
    "UserKnownHostsFile=/lease/known_hosts",
    "GlobalKnownHostsFile=none",
    "HostKeyAlias=crabbox-lease-1f2e",
];

pub(in crate::adapters::crabbox) struct Lab {
    pub(in crate::adapters::crabbox) dir: TempDir,
    pub(in crate::adapters::crabbox) cb: Crabbox,
}

pub(in crate::adapters::crabbox) fn lab() -> Lab {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    fs::create_dir_all(bin.join("boxtmp")).unwrap();
    for (name, text) in [("ssh", FAKE_SSH), ("crabbox", FAKE_CRABBOX)] {
        moor_testkit::write_executable(&bin.join(name), text).unwrap();
    }
    let cb = driver(dir.path(), &tools(&bin));
    Lab { dir, cb }
}

/// The fake tools in `bin`.
pub(in crate::adapters::crabbox) fn tools(bin: &Path) -> ToolPaths {
    ToolPaths {
        ssh: Some(bin.join("ssh")),
        crabbox: Some(bin.join("crabbox")),
        ..ToolPaths::default()
    }
}

/// A driver over real processes keeping its state and sockets under `dir`.
pub(in crate::adapters::crabbox) fn driver(dir: &Path, tools: &ToolPaths) -> Crabbox {
    let proc = Arc::new(SystemProc::new(
        &Env::from_pairs([("PATH", "/usr/bin:/bin")]),
        tools,
    ));
    let clock = Arc::new(FakeClock::new(moor_core::seam::clock::Ts::from_millis(0)));
    let cb = Crabbox::new(proc, clock, Arc::new(FakeRng::default()))
        .with_state_dir(Some(dir.join("state")), dir.join("s"));
    let _ = cb.version_ok.set(());
    cb
}

pub(in crate::adapters::crabbox) fn handle() -> BoxHandle {
    BoxHandle::new("external/moor-7-ab12cd")
}

impl Lab {
    pub(in crate::adapters::crabbox) fn bin(&self) -> PathBuf {
        self.dir.path().join("bin")
    }

    /// Every recorded argv of `tool`, in call order.
    pub(in crate::adapters::crabbox) fn argvs(&self, tool: &str) -> Vec<Vec<String>> {
        (0..100)
            .map(|n| self.bin().join(format!("{tool}.{n}")))
            .take_while(|p| p.exists())
            .map(|p| {
                let raw = fs::read_to_string(p).unwrap();
                raw.split_terminator('\0').map(str::to_owned).collect()
            })
            .collect()
    }

    /// `argv` opens or joins the Box's master in moor's socket dir, and keeps crabbox's pinning.
    pub(in crate::adapters::crabbox) fn assert_on_the_master_and_pinned(&self, argv: &[String]) {
        let socket = format!("ControlPath={}", self.dir.path().join("s").display());
        let first_path = argv.iter().find(|a| a.starts_with("ControlPath=")).unwrap();
        assert!(first_path.starts_with(&socket), "{argv:?}");
        assert!(argv.iter().any(|a| a == "ControlMaster=auto"), "{argv:?}");
        for word in PINNED {
            assert!(argv.iter().any(|a| a == word), "{word} missing: {argv:?}");
        }
    }
}
