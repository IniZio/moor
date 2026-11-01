//! Making room on a full node by suspending its longest-idle Boxes.

use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::name::BoxId;
use crate::seam::clock::Ts;

/// A Box on the node, as the scheduler sees it when the node is short of memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resident {
    /// The Box.
    pub id: BoxId,
    /// Its lifecycle.
    pub lifecycle: BoxLifecycle,
    /// Its primary Session.
    pub session: SessionState,
    /// Whether its agent waits on a question.
    pub needs_input: bool,
    /// When it last became Idle.
    pub idle_since: Option<Ts>,
    /// The guest memory it holds, in MiB.
    pub mem_mib: u64,
}

/// How a node short of memory makes room for one more Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Room {
    /// It fits without suspending anything.
    Fits,
    /// Suspending these Boxes, longest idle first, frees enough.
    Suspend(Vec<BoxId>),
    /// Even suspending every idle Box would not free enough, so none is suspended.
    Short,
}

/// Whether suspending `r` cannot lose work: it is up and its agent waits on nothing.
#[must_use]
pub fn suspendable(r: &Resident) -> bool {
    r.lifecycle == BoxLifecycle::Ready
        && r.session == SessionState::Idle
        && !r.needs_input
        && r.idle_since.is_some()
}

/// Picks the fewest longest-idle Boxes whose memory lets `need_mib` fit in `free_mib`.
#[must_use]
pub fn make_room(free_mib: u64, need_mib: u64, residents: &[Resident]) -> Room {
    if free_mib >= need_mib {
        return Room::Fits;
    }
    let mut idle: Vec<&Resident> = residents.iter().filter(|r| suspendable(r)).collect();
    idle.sort_by_key(|r| (r.idle_since.map(Ts::as_millis), r.id));
    let mut free = free_mib;
    let mut picked = Vec::new();
    for r in idle {
        picked.push(r.id);
        free = free.saturating_add(r.mem_mib);
        if free >= need_mib {
            return Room::Suspend(picked);
        }
    }
    Room::Short
}

#[cfg(test)]
mod tests;
