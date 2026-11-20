//! Every Box on this machine, as each place stands now.

use super::args::{PLACE_LABEL, SLUG_LABEL};
use super::cli::{self, parse_ps, Cli};
use super::names::Names;
use super::Docker;
use crate::boxes::provider::{Observed, PlaceState, ProvideError};

impl Docker {
    /// Every Box's places as `docker ps` and `inspect` say they stand.
    pub(super) async fn observe_all(&self) -> Result<Vec<Observed>, ProvideError> {
        let format = format!(
            "{{{{.Label \"{SLUG_LABEL}\"}}}} {{{{.Label \"{PLACE_LABEL}\"}}}} {{{{.State}}}}"
        );
        let listed = Cli::ok([
            "ps",
            "-a",
            "--filter",
            &format!("label={SLUG_LABEL}"),
            "--format",
            &format,
        ])
        .await?;
        let mut boxes = Vec::new();
        let listed = parse_ps(&listed);
        self.egress
            .keep_only(&listed.keys().cloned().collect::<Vec<_>>());
        let ended = cli::ended(&listed).await;
        let ended_as = |name: &str, place| match ended.get(name) {
            Some(e) if place == PlaceState::Down => PlaceState::Exited(*e),
            _ => place,
        };
        for (slug, (agent, work)) in listed {
            let names = Names::of(&slug);
            let (agent, work) = (ended_as(&names.agent, agent), ended_as(&names.work, work));
            let oom_kills = if agent == PlaceState::Up {
                self.egress.read_when_due(&slug, &self.settings.agent_image);
                self.oom_kills(&names).await
            } else {
                0
            };
            boxes.push(Observed {
                slug,
                agent,
                work,
                oom_kills,
                mem_mib: None,
            });
        }
        Ok(boxes)
    }
}
