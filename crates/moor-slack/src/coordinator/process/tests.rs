//! Tests for one supervised process's output.

use std::process::Command;
use std::time::Duration;

use super::{now, supervise, Left, Next, MAX_OUTPUT};

#[test]
fn a_line_with_no_end_is_cut_at_the_cap() {
    let mut cmd = Command::new("sh");
    cmd.args([
        "-c",
        "head -c 3000000 /dev/zero | tr '\\0' a; echo; echo next",
    ]);
    let deadline = now() + Duration::from_secs(30);
    let (lines, _) = supervise(cmd, b"", None, |lines| {
        let mut seen = Vec::new();
        while let Next::Line(line) = lines.next(deadline) {
            seen.push(line);
        }
        (seen, Left::Ended)
    })
    .unwrap();
    let lens: Vec<usize> = lines.iter().map(String::len).collect();
    assert_eq!(lens, [MAX_OUTPUT]);
}
