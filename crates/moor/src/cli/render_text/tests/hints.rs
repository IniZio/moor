//! The next steps `start` prints are commands moor takes.

use moor_core::Response;
use serde_json::json;

use super::lifecycle_box;
use crate::cli::render_text::render;

#[test]
fn every_command_start_suggests_is_one_moor_parses() {
    let v = json!({"box": lifecycle_box("alpha", "ready", Some("ready")), "attached": false});
    let (out, _) = render(&Response::ok("start", Some(v)), false);
    let hints: Vec<&str> = out
        .lines()
        .filter_map(|l| l.split_once("with: ").map(|(_, cmd)| cmd))
        .collect();
    assert!(!hints.is_empty(), "start says what to do next: {out}");
    for cmd in hints {
        let argv = cmd
            .split_whitespace()
            .map(std::ffi::OsString::from)
            .collect();
        assert!(
            crate::cli::args::parse(argv).is_ok(),
            "`{cmd}` is not a moor command"
        );
    }
}
