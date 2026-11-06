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

#[test]
fn a_dial_takes_a_timeout_and_still_refuses_an_unknown_field() {
    use crate::link::broker::PoolOp;
    let plain = r#"{"op":"dial","slug":"moor-1a2b3c4d-7"}"#;
    let timed = r#"{"op":"dial","slug":"moor-1a2b3c4d-7","timeout_secs":20}"#;
    let stray = r#"{"op":"dial","slug":"moor-1a2b3c4d-7","deadline":20}"#;
    for (json, sent) in [(plain, None), (timed, Some(20))] {
        let op: super::Op = serde_json::from_str(json).unwrap();
        let back = serde_json::to_value(&op).unwrap();
        assert_eq!(
            back.get("timeout_secs").and_then(serde_json::Value::as_u64),
            sent
        );
        assert_eq!(serde_json::to_string(&op).unwrap(), json);
        let pooled: PoolOp = serde_json::from_str(json).unwrap();
        assert_eq!(super::Op::from(pooled), op);
    }
    assert!(serde_json::from_str::<super::Op>(stray).is_err());
    assert!(serde_json::from_str::<PoolOp>(stray).is_err());
}

#[test]
fn a_checkpoint_op_carries_one_verb_and_refuses_a_stray_field() {
    use crate::link::broker::PoolOp;
    use crate::link::lease::{Answer, CheckpointAct};
    let acts = [
        (
            r#"{"op":"checkpoint","act":{"verb":"checkpoint-create","slug":"moor-1a2b3c4d-7","name":"n"}}"#,
            CheckpointAct::CheckpointCreate {
                slug: "moor-1a2b3c4d-7".to_owned(),
                name: "n".to_owned(),
            },
        ),
        (
            r#"{"op":"checkpoint","act":{"verb":"checkpoint-fork","id":"chk_1","slug":"moor-1a2b3c4d-7"}}"#,
            CheckpointAct::CheckpointFork {
                id: "chk_1".to_owned(),
                slug: "moor-1a2b3c4d-7".to_owned(),
                cache: None,
                class: None,
            },
        ),
        (
            r#"{"op":"checkpoint","act":{"verb":"checkpoint-delete","id":"chk_1"}}"#,
            CheckpointAct::CheckpointDelete {
                id: "chk_1".to_owned(),
            },
        ),
        (
            r#"{"op":"checkpoint","act":{"verb":"disk"}}"#,
            CheckpointAct::Disk {},
        ),
    ];
    for (json, act) in acts {
        let op = super::Op::Checkpoint { act };
        assert_eq!(serde_json::to_string(&op).unwrap(), json);
        assert_eq!(serde_json::from_str::<super::Op>(json).unwrap(), op);
        let pooled: PoolOp = serde_json::from_str(json).unwrap();
        assert_eq!(super::Op::from(pooled), op);
    }
    let stray = r#"{"op":"checkpoint","act":{"verb":"disk","slug":"x"}}"#;
    assert!(serde_json::from_str::<super::Op>(stray).is_err());
    assert!(serde_json::from_str::<PoolOp>(stray).is_err());
    let answer = Answer::Disk {
        free_mib: 1,
        total_mib: 2,
    };
    let reply = super::Reply::Leased(answer);
    let text = serde_json::to_string(&reply).unwrap();
    assert_eq!(serde_json::from_str::<super::Reply>(&text).unwrap(), reply);
}
