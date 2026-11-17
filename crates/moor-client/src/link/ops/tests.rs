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
    let mut child = moor_testkit::ChildGuard::new(
        Command::new("bash")
            .args(["-c", &in_box(7, "sleep 5 >/dev/null 2>&1 & echo started")])
            .env("HOME", home.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
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

#[test]
fn a_cache_carries_its_volumes_both_ways() {
    let cache = crate::link::lease::Cache {
        volumes: vec![crate::link::lease::CacheVolume {
            group: "0123456789abcdef".to_owned(),
            name: "cargo".to_owned(),
            key: "0123456789abcdef-cargo".to_owned(),
            paths: vec!["~/.cargo/registry".to_owned()],
            size_gb: 4,
        }],
        scope: Some("0123456789abcdef".to_owned()),
    };
    let json = serde_json::to_string(&cache).unwrap();
    assert!(json.contains(r#""sizeGB":4"#), "{json}");
    assert_eq!(
        serde_json::from_str::<crate::link::lease::Cache>(&json).unwrap(),
        cache
    );
}
