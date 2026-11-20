//! When each canonical cache volume was last used, kept in `<home>/cache-use.json`.
//!
//! A Docker label cannot change after a volume is made, so the machine agent keeps the times
//! itself. A Box's open and its save both count as use.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

/// The last use of each canonical volume, in Unix seconds, by volume name.
#[derive(Default)]
pub(in crate::boxes::docker) struct Uses {
    /// Where it is kept; none keeps it in memory only.
    file: Mutex<(Option<PathBuf>, BTreeMap<String, i64>)>,
}

impl Uses {
    /// Keeps the times in `file` from now on, reading what it holds.
    pub(in crate::boxes::docker) fn keep_in(&self, file: PathBuf) {
        let held = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        *self.file.lock().unwrap_or_else(PoisonError::into_inner) = (Some(file), held);
    }

    /// Writes the times; a failed write keeps them in memory, and the next change tries again.
    fn write(state: &(Option<PathBuf>, BTreeMap<String, i64>)) {
        let (Some(file), times) = state else {
            return;
        };
        let temp = file.with_extension("json.new");
        let text = serde_json::to_string(times).unwrap_or_default();
        if std::fs::write(&temp, text).is_ok() {
            drop(std::fs::rename(&temp, file));
        }
    }

    /// Notes that each of `names` is used now.
    pub(in crate::boxes::docker) fn touch<'a>(&self, names: impl IntoIterator<Item = &'a str>) {
        self.touch_at(names, crate::unix_now());
    }

    /// Notes that each of `names` was used at `now`.
    pub(in crate::boxes::docker) fn touch_at<'a>(
        &self,
        names: impl IntoIterator<Item = &'a str>,
        now: i64,
    ) {
        let mut state = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        for name in names {
            state.1.insert(name.to_owned(), now);
        }
        Self::write(&state);
        drop(state);
    }

    /// When `name` was last used.
    pub(in crate::boxes::docker) fn last(&self, name: &str) -> Option<i64> {
        let state = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        state.1.get(name).copied()
    }

    /// Forgets `name`, whose volume is gone.
    pub(in crate::boxes::docker) fn forget(&self, name: &str) {
        let mut state = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        if state.1.remove(name).is_some() {
            Self::write(&state);
        }
    }
}
