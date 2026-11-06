//! A stopping runner hands its links over: each agent is told to dial again.

use std::sync::PoisonError;
use std::time::Duration;

use moor_client::link::Frame;

use super::Agents;

/// How long answers read off a drained link's last streams get to reach their callers.
const SETTLE: Duration = Duration::from_millis(500);

impl Agents {
    /// Tells every linked agent, and every agent that dials from now on, to dial again; resolves
    /// once each link has ended, or after `budget`. An agent ends its old link when its streams
    /// finish.
    pub async fn drain(&self, budget: Duration) {
        let mut report = self.links.report.subscribe();
        self.links.stopping.send_replace(true);
        for h in self
            .links
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            if let Some(tell) = &h.tell {
                let _ = tell.try_send(Frame::Relink {});
            }
        }
        let ended = report.wait_for(|r| r.links.iter().all(|l| !l.online));
        if tokio::time::timeout(budget, ended).await.is_ok() {
            tokio::time::sleep(SETTLE).await;
        }
    }
}
