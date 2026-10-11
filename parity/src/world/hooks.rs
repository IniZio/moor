use super::World;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookState {
    Starting,
    Working,
    Idle,
    Blocked,
}

impl HookState {
    fn raw(self) -> &'static str {
        match self {
            HookState::Starting => "starting",
            HookState::Working => "working",
            HookState::Idle => "idle",
            HookState::Blocked => "waiting_input",
        }
    }
}

impl World {
    pub fn hook_state_path(&self) -> std::path::PathBuf {
        self.box_root().join("work/.moor/agent-state.json")
    }

    pub fn hook_state(&self, state: HookState, age_secs: u64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let doc = serde_json::json!({
            "state": state.raw(),
            "event": "parity",
            "ts": now.saturating_sub(age_secs),
        });
        let path = self.hook_state_path();
        fs::create_dir_all(path.parent().expect("hook dir")).expect("create hook dir");
        fs::write(path, doc.to_string()).expect("write hook state");
    }

    pub fn clear_hook_state(&self) {
        let _ = fs::remove_file(self.hook_state_path());
    }
}
