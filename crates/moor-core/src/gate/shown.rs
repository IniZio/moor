//! The gates as a run page shows them: one step each, in the shape of a plan's steps.

use serde_json::{json, Value};

use super::{Approval, Evidence, Gates};
use crate::workflow::EvidenceName;

impl Gates {
    /// The gates as the steps of a plan document, for a page that lists a run's steps: each gate
    /// is a step of kind `gate` named for it, with what it asks as the step's `done_when`.
    #[must_use]
    pub fn steps_doc(&self) -> Value {
        let steps: Vec<Value> = self
            .iter()
            .map(|gate| {
                let files: Vec<&str> = gate
                    .evidence
                    .iter()
                    .flat_map(Evidence::names)
                    .map(EvidenceName::as_str)
                    .collect();
                let runs: Vec<&str> = gate.checks.iter().map(|c| c.run.as_str()).collect();
                let mut asks = Vec::new();
                if !files.is_empty() {
                    asks.push(format!("the agent reports {}", files.join(", ")));
                }
                if !gate.checks.is_empty() {
                    asks.push("its checks exit 0".to_owned());
                }
                if gate.approval == Approval::Owner {
                    asks.push("the owner approves".to_owned());
                }
                json!({
                    "id": gate.name.as_str(),
                    "title": gate.name.as_str(),
                    "kind": "gate",
                    "done_when": asks.join("; "),
                    "command": Some(runs.join("; ")).filter(|c| !c.is_empty()),
                    "checks": gate.checks.iter().map(|c| json!({"run": c.run, "says": c.says})).collect::<Vec<_>>(),
                    "evidence": files,
                    "max_tries": gate.max_tries.get(),
                })
            })
            .collect();
        json!({ "steps": steps })
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::indexing_slicing,
        reason = "a test indexes the JSON it just built"
    )]

    use crate::gate::{Approval, GateDoc, Gates};

    #[test]
    fn each_gate_is_a_step_named_for_it() {
        let gates = Gates::parse(
            &[
                GateDoc {
                    name: "proof".to_owned(),
                    evidence: vec!["pr-*".to_owned()],
                    checks: Vec::new(),
                    approval: None,
                    max_tries: None,
                },
                GateDoc {
                    name: "done".to_owned(),
                    evidence: vec!["posted.md".to_owned()],
                    checks: vec!["true".into()],
                    approval: Some(Approval::Owner),
                    max_tries: Some(1),
                },
            ],
            &[],
        )
        .unwrap();
        let doc = gates.steps_doc();
        let steps = doc["steps"].as_array().unwrap();
        let ids: Vec<_> = steps.iter().map(|s| s["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["proof", "done"]);
        assert_eq!(steps[0]["done_when"], "the agent reports pr-*");
        assert_eq!(steps[0]["command"], serde_json::Value::Null);
        assert_eq!(
            steps[1]["done_when"],
            "the agent reports posted.md; its checks exit 0; the owner approves"
        );
        assert_eq!(steps[1]["command"], "true");
        assert_eq!(
            (
                steps[0]["max_tries"].as_u64(),
                steps[1]["max_tries"].as_u64()
            ),
            (Some(2), Some(1))
        );
        assert!(!doc.to_string().contains("deliver"));
    }
}
