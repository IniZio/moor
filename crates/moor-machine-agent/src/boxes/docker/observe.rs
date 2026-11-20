//! Every Box on this machine, as each place stands now, and the host memory each holds.

use std::collections::BTreeMap;

use bytesize::ByteSize;
use serde::Deserialize;

use super::args::{PLACE_LABEL, SLUG_LABEL};
use super::cli::{self, parse_ps, Cli};
use super::names::Names;
use super::Docker;
use crate::boxes::provider::{Observed, PlaceState, ProvideError};
use crate::boxes::Slug;

/// One running container as `docker stats --format '{{json .}}'` gives it.
#[derive(Debug, Deserialize)]
struct Stats {
    /// The container's name.
    #[serde(rename = "Name")]
    name: String,
    /// What it uses and its limit, such as `3.9GiB / 16GiB`. Docker counts what the cgroup holds
    /// less its inactive file cache, which the kernel takes back first.
    #[serde(rename = "MemUsage")]
    mem_usage: String,
}

/// MiB of the host each Box holds, both places, from `docker stats` lines; a line for a container
/// that is not a Box place, or that cannot be read, counts for nothing.
pub(super) fn held_by_box(stats: &str) -> BTreeMap<Slug, u64> {
    let mut held = BTreeMap::new();
    for line in stats.lines() {
        let Ok(s) = serde_json::from_str::<Stats>(line) else {
            continue;
        };
        let slug = ["-agent", "-work"]
            .iter()
            .find_map(|place| s.name.strip_suffix(place))
            .and_then(Slug::parse);
        let used = s.mem_usage.split(" / ").next().map(str::trim);
        let bytes = used.and_then(|u| u.parse::<ByteSize>().ok());
        if let (Some(slug), Some(bytes)) = (slug, bytes) {
            *held.entry(slug).or_default() += bytes.as_u64() / (1024 * 1024);
        }
    }
    held
}

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

    /// MiB of the host each running Box holds, from one `docker stats` of every running container;
    /// none when Docker cannot say.
    pub(super) async fn held_mib_all() -> BTreeMap<Slug, u64> {
        Cli::ok(["stats", "--no-stream", "--format", "{{json .}}"])
            .await
            .map(|out| held_by_box(&out))
            .unwrap_or_default()
    }
}
