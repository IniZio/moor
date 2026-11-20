//! A Box's health, as the agent reports it, from what is held and what the provider sees.

use moor_client::link::boxes::{BoxReport, ReportState};

use super::held::{Held, Want};
use super::provider::{Ended, Observed, PlaceState};
use super::Slug;

/// One Box's health from what is held and what the provider sees, or why it lost every Box.
pub(super) fn report(
    slug: &Slug,
    held: &Held,
    seen: Option<&Observed>,
    lost: Option<&str>,
) -> BoxReport {
    let up = seen.is_some_and(|o| o.agent == PlaceState::Up && o.work == PlaceState::Up);
    let state = match (&held.broken, held.want) {
        (Some(why), _) => ReportState::Broken { why: why.clone() },
        (None, Want::Asleep) => ReportState::Asleep,
        (None, Want::Awake) if up => ReportState::Awake,
        (None, Want::Awake) => ReportState::Mending,
    };
    let down = match (&state, seen) {
        (ReportState::Mending, Some(o)) => {
            let mut said: Vec<String> = [o.agent, o.work]
                .into_iter()
                .filter_map(place_down)
                .collect();
            said.dedup();
            Some(said.join("; "))
        }
        (ReportState::Mending, None) => lost.map(str::to_owned),
        _ => None,
    };
    BoxReport {
        slug: slug.as_str().to_owned(),
        state,
        generation: held.generation,
        oom_kills: seen.map_or(0, |o| o.oom_kills),
        last_mend: None,
        mem_mib: seen.and_then(|o| o.mem_mib),
        down,
    }
}

/// How a place is down, worded for people; none when it runs.
fn place_down(place: PlaceState) -> Option<String> {
    Some(match place {
        PlaceState::Up => return None,
        PlaceState::Down => "the Box's container is not running".to_owned(),
        PlaceState::Exited(Ended { code, oom: true }) => {
            format!("the Box's container ran out of memory (exit {code})")
        }
        PlaceState::Exited(Ended { code, oom: false }) => {
            format!("the Box's container stopped (exit {code})")
        }
    })
}
