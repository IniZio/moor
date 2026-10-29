//! How the `box_*` events move a status view's machine line.

use serde_json::Value;

use super::{Machine, View};

impl View {
    /// Sets the machine and its note the `box_*` event of `kind` leads to.
    pub(super) fn apply_machine(&mut self, kind: &str, payload: &Value) {
        let note = |key: &str| {
            payload
                .get(key)
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
        };
        match kind {
            "box_requested" => {
                self.machine = Some(Machine::Requested);
                self.machine_note = None;
            }
            "box_progress" => self.machine_note = note("text"),
            "box_ready" => {
                self.machine = Some(Machine::Ready);
                self.machine_note = None;
            }
            "box_failed" => {
                self.machine = Some(Machine::Failed);
                self.machine_note = note("error");
            }
            "box_asleep" => self.machine = Some(Machine::Asleep),
            "box_gone" => self.machine = Some(Machine::Gone),
            "box_deleted" => self.machine = Some(Machine::Deleted),
            _ => {}
        }
    }
}
