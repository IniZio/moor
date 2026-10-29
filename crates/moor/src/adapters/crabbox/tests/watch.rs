//! A real crabbox child whose acquire is still waiting when its first lines reach moor.

use std::fs;

use moor_core::seam::driver::Driver;

use super::fake_ssh::{driver, tools};
use super::{req, WARMUP_OUT};

/// Prints a lease and an SSH wait on stderr, then waits up to 10s for a `go` file before it
/// prints the warmup result and leaves a `done` file.
const SLOW_CRABBOX: &str = r#"#!/bin/sh
d=$(dirname "$0")
echo "leased lease-7f3a slug=box-a server=12 type=small ip=10.0.0.9 via coordinator" >&2
echo "waiting for 10.0.0.9:22 external provider SSH... elapsed=0s remaining=20m0s ports=22:closed" >&2
i=0
while ! test -f "$d/go" && test $i -lt 100; do sleep 0.1; i=$((i+1)); done
cat "$d/out"
touch "$d/done"
"#;

#[test]
fn acquire_notes_arrive_while_crabbox_still_runs() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    moor_testkit::write_executable(&bin.join("crabbox"), SLOW_CRABBOX).unwrap();
    fs::write(bin.join("out"), WARMUP_OUT).unwrap();
    let cb = driver(dir.path(), &tools(&bin));
    let mut got = Vec::new();
    cb.acquire_noting(&req("external", None), &mut |n| {
        got.push((n.to_owned(), bin.join("done").exists()));
        if got.len() == 2 {
            fs::write(bin.join("go"), "").unwrap();
        }
    })
    .unwrap();
    assert_eq!(
        got,
        [
            ("machine created".to_owned(), false),
            ("waiting for SSH: port 22 closed".to_owned(), false),
        ]
    );
}
