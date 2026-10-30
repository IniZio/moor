use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use super::in_box;

/// ssh ends a call only once the remote side closes its output, so that is what must happen
/// while the job still runs.
#[test]
fn in_box_closes_its_output_while_a_job_its_body_started_runs_on() {
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new("bash")
        .args(["-c", &in_box(7, "sleep 5 >/dev/null 2>&1 & echo started")])
        .env("HOME", home.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        let _ = tx.send(out);
    });
    let out = rx.recv_timeout(Duration::from_secs(3));
    let _ = child.wait();
    assert_eq!(out.as_deref(), Ok("started\n"));
    assert!(home.path().join("moor/7").is_dir());
}
