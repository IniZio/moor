//! What a work Box receives at launch: moor's contract for its situation, and no old Box text.

use moor_core::seam::engine::RunLaunch;

use super::*;
use crate::testing::contract::{locked_version, OLD_TEXT};

/// A run's launch: its link, a failed setup, and a first message that names its gates.
fn run_req() -> LaunchReq {
    let doc: moor_core::gate::GatesDoc = serde_json::from_value(serde_json::json!({
        "gates": [{"name": "proof", "evidence": ["proof.md"], "checks": ["true"]}],
        "outcomes": [
            {"name": "nothing to do", "checks": ["true"], "anytime": true},
            {"name": "done", "checks": ["true"]}
        ]
    }))
    .unwrap();
    let gates = moor_core::gate::Gates::parse(&doc.gates, &doc.outcomes).unwrap();
    LaunchReq {
        run: Some(RunLaunch {
            link: Some("https://moor.example/#/runs/7".to_owned()),
        }),
        setup_exit: Some(1),
        prompt: LaunchPrompt::Text(gates.first_message("Collect evidence.")),
        ..req()
    }
}

/// The brief `r`'s launch put in the Box.
fn brief_of(r: &LaunchReq) -> String {
    let d = driver(FakeDriver::new());
    agent(&d).launch(r).unwrap();
    let brief = format!("{CRED}/brief.md");
    let (_, _, bytes, _) = d
        .puts()
        .into_iter()
        .find(|(_, path, _, _)| path.as_str() == brief)
        .expect("the launch puts a brief");
    String::from_utf8(bytes).unwrap()
}

#[test]
fn a_run_s_launch_gives_the_locked_contract_with_its_run_part_and_a_task_s_without() {
    let head = format!("# moor agent contract\n\nVersion {}\n", locked_version());
    let run = brief_of(&run_req());
    assert!(
        run.starts_with(&head) && run.contains("\n## run\n"),
        "{run}"
    );
    let task = brief_of(&req());
    assert!(
        task.starts_with(&head) && !task.contains("\n## run\n"),
        "{task}"
    );
}

#[test]
fn no_old_task_or_run_box_text_reaches_the_box_by_a_launch() {
    for r in [
        LaunchReq {
            prompt: LaunchPrompt::Text("Fix the typo.".to_owned()),
            issue: Some("x is broken".to_owned()),
            ..req()
        },
        run_req(),
    ] {
        let d = driver(FakeDriver::new());
        agent(&d).launch(&r).unwrap();
        let mut sent: Vec<String> = d
            .puts()
            .into_iter()
            .map(|(_, path, bytes, _)| format!("{path:?}: {}", String::from_utf8_lossy(&bytes)))
            .collect();
        sent.extend(d.execs().into_iter().map(|(_, spec, _)| {
            let env = spec.env.iter().map(|(k, v)| format!("{k}={v}"));
            spec.argv
                .into_iter()
                .chain(env)
                .collect::<Vec<_>>()
                .join("\n")
        }));
        for text in &sent {
            for old in OLD_TEXT {
                assert!(!text.contains(old), "{old:?} reaches the Box:\n{text}");
            }
        }
    }
}
