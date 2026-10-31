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

fn warmup(cache: Option<super::lease::Cache>) -> super::Op {
    super::Op::Lease {
        verb: super::lease::Verb::Warmup,
        slug: Some("moor-1a2b3c4d-7".to_owned()),
        key: None,
        cache,
        class: None,
    }
}

#[test]
fn a_lease_without_cache_reads_as_an_older_agent_wrote_it() {
    let old = r#"{"op":"lease","verb":"warmup","slug":"moor-1a2b3c4d-7"}"#;
    assert_eq!(
        serde_json::from_str::<super::Op>(old).unwrap(),
        warmup(None)
    );
    assert_eq!(serde_json::to_string(&warmup(None)).unwrap(), old);
}

#[test]
fn a_lease_carries_its_cache_both_ways() {
    let op = warmup(Some(super::lease::Cache {
        volumes: vec![super::lease::CacheVolume {
            group: "0123456789abcdef".to_owned(),
            name: "cargo".to_owned(),
            key: "0123456789abcdef-cargo".to_owned(),
            paths: vec!["~/.cargo/registry".to_owned()],
            size_gb: 4,
            sealed: true,
        }],
        scope: Some("0123456789abcdef".to_owned()),
    }));
    let json = serde_json::to_string(&op).unwrap();
    assert!(json.contains(r#""sizeGB":4"#), "{json}");
    assert_eq!(serde_json::from_str::<super::Op>(&json).unwrap(), op);
}

#[test]
fn a_warmup_carries_its_class_and_names_none_without_one() {
    let mut op = warmup(None);
    if let super::Op::Lease { class, .. } = &mut op {
        *class = Some("fast".to_owned());
    }
    let json = serde_json::to_string(&op).unwrap();
    assert!(json.ends_with(r#","class":"fast"}"#), "{json}");
    assert_eq!(serde_json::from_str::<super::Op>(&json).unwrap(), op);
    assert!(!serde_json::to_string(&warmup(None))
        .unwrap()
        .contains("class"));
}
