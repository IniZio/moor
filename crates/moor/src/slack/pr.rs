//! Per-task cache of the open pull request link shown in status messages.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::status::{State, View};
use crate::store::SlackThread;

/// Least time between two lookups for one task.
pub(super) const REFRESH_EVERY: Duration = Duration::from_secs(60);

/// Finds the open pull request URL of a task's branch; `None` when absent or on any error.
pub(crate) type PrOf = dyn Fn(&str) -> Option<String> + Send + Sync;

/// Pull request URLs by task name, refreshed lazily.
#[derive(Default)]
pub(super) struct PrCache {
    /// Last URL found per task.
    urls: HashMap<String, String>,
    /// When each task was last looked up.
    checked: HashMap<String, Instant>,
}

impl PrCache {
    /// Looks up tasks that have a thread, are not closed and were last checked over a minute ago.
    ///
    /// A failed or empty lookup keeps the URL already known.
    pub(super) fn refresh(
        &mut self,
        threads: &[SlackThread],
        views: &HashMap<String, View>,
        pr_of: &PrOf,
        now: Instant,
    ) {
        for t in threads {
            let task = t.task.as_str();
            if views.get(task).is_some_and(|v| v.state == State::Closed) {
                continue;
            }
            if self
                .checked
                .get(task)
                .is_some_and(|at| now.duration_since(*at) < REFRESH_EVERY)
            {
                continue;
            }
            self.checked.insert(task.to_owned(), now);
            if let Some(url) = pr_of(task) {
                self.urls.insert(task.to_owned(), url);
            }
        }
    }

    /// The known URLs by task name.
    pub(super) const fn urls(&self) -> &HashMap<String, String> {
        &self.urls
    }
}

#[cfg(test)]
mod tests;
