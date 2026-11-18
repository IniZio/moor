//! A stand-in for the controller's `machine-pair` request, for gate tests whose fake controller
//! answers it: codes kept in memory, and nothing expires. The controller's own tests cover expiry.

use std::collections::HashMap;

use serde_json::{json, Value};

/// One code.
struct Code {
    /// The secret the agent polls with.
    device: String,
    /// What the agent asked for.
    ask: Value,
    /// `waiting`, `enrolling` or `enrolled`.
    stage: &'static str,
    /// The enrollment, once settled.
    enrolled: Value,
}

/// The codes, by code.
#[derive(Default)]
pub struct Pairings(HashMap<String, Code>);

/// `op`'s string field `key`; empty when it has none.
fn field<'a>(op: &'a Value, key: &str) -> &'a str {
    op.get(key).and_then(Value::as_str).unwrap_or_default()
}

impl Pairings {
    /// Answers one `machine-pair` op, the request's `op` object, the way the controller does.
    pub fn step(&mut self, op: &Value) -> Value {
        let gone = json!({"pairing": "gone"});
        let code = field(op, "code").to_owned();
        let waiting = self.0.get_mut(&code).filter(|c| c.stage == "waiting");
        match field(op, "do") {
            "start" => {
                let pairing = Code {
                    device: field(op, "device_code").to_owned(),
                    ask: op.get("ask").cloned().unwrap_or_default(),
                    stage: "waiting",
                    enrolled: Value::Null,
                };
                self.0.insert(code, pairing);
                json!({"pairing": "started"})
            }
            "show" => waiting.map_or(gone, |c| json!({"pairing": "waiting", "ask": c.ask})),
            "claim" => waiting.map_or(gone, |c| {
                c.stage = "enrolling";
                json!({"pairing": "waiting", "ask": c.ask})
            }),
            "settle" => {
                let enrolled = op.get("enrolled").cloned().unwrap_or_default();
                let Some(c) = self.0.get_mut(&code).filter(|c| c.stage == "enrolling") else {
                    return gone;
                };
                c.stage = if enrolled.is_null() {
                    "waiting"
                } else {
                    "enrolled"
                };
                c.enrolled = enrolled;
                json!({"pairing": "settled"})
            }
            "poll" => self.poll(field(op, "device_code")),
            _ => gone,
        }
    }

    /// What a poll with `device` finds; an enrollment is handed out once.
    fn poll(&mut self, device: &str) -> Value {
        let Some((code, stage)) = self
            .0
            .iter()
            .find(|(_, c)| c.device == device)
            .map(|(code, c)| (code.clone(), c.stage))
        else {
            return json!({"pairing": "gone"});
        };
        if stage != "enrolled" {
            return json!({"pairing": "pending"});
        }
        let enrolled = self.0.remove(&code).map(|c| c.enrolled).unwrap_or_default();
        json!({"pairing": "enrolled", "enrolled": enrolled})
    }
}
