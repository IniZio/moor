//! The one gate every resume passes, and the queue on a node it keeps: Boxes suspended as a last
//! resort, then other resumes, then new starts.

use moor_core::decide::pressure::Demand;
use moor_core::decide::schedule::{needed_to_resume, Memory, Node, ResumeAsk, Shortfall};
use moor_core::domain::boxes::BoxState;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::node::{now, recorded};
use crate::ops::common::room::{helps, suspend_idlest};
use crate::store::{BoxRow, Store};

/// How a resuming Box stands to its node's queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Turn {
    /// A promotion reached it, going down the queue.
    Reached,
    /// Something asked for it now, so it goes behind whatever the queue holds ahead of it.
    JoinsQueue,
    /// A close wants it awake just long enough to check it. That is no demand to run: it wakes
    /// only if it fits now, and otherwise joins no queue and costs no other Box its room.
    OnlyIfRoom,
}

/// How many Boxes in the queue on `node` stand ahead of `row`. One not in the queue yet goes after
/// its resumes and before its new starts.
fn ahead_of(store: &Store, node: &Node, row: &BoxRow) -> Result<usize, CommandError> {
    let queue = store.pending_on(&node.name)?;
    if let Some(at) = queue.iter().position(|id| *id == row.id) {
        return Ok(at);
    }
    let mut resumes = 0;
    for id in queue {
        let asleep = store
            .box_row(id)?
            .and_then(|r| r.boxed)
            .is_some_and(|b| b.state == BoxState::Asleep);
        resumes += usize::from(asleep);
    }
    Ok(resumes)
}

/// Why asleep `row` does not resume on `node` at `now`; `None` when it may. Every resume is judged
/// here, promotions and what a person or agent asks for alike.
fn resume_shortfall(
    store: &Store,
    node: &Node,
    memory: &Memory,
    row: &BoxRow,
    now: i64,
) -> Result<Option<Shortfall>, CommandError> {
    let guest = row
        .boxed
        .as_ref()
        .and_then(|b| b.delivered)
        .map(|d| d.mem_mib);
    let reported = store.resume_memory_mib(row.id)?;
    let ready = store.ready_on(&node.name)?;
    let mut unreported = false;
    for id in &ready {
        let up = store.awake_since(*id)?.map(|ts| ts.as_millis() / 1000);
        unreported |= up.is_some_and(|at| at > memory.reported_at);
    }
    Ok(memory.resume_shortfall(
        now,
        ResumeAsk {
            needed_mib: needed_to_resume(reported, guest),
            running: ready.len() + memory.starting,
            unreported,
        },
    ))
}

/// Whether asleep `row` could not fit on `node` even with every Box running there gone. Boxes behind
/// it may then pass, and none is suspended for it. As soon as stopping what runs would make room,
/// neither holds, so passing Boxes cannot starve it.
pub(crate) fn beyond_reach(
    store: &Store,
    node: &Node,
    memory: &Memory,
    row: &BoxRow,
) -> Result<bool, CommandError> {
    let Some(b) = row.boxed.as_ref().filter(|b| b.state == BoxState::Asleep) else {
        return Ok(false);
    };
    let guest = b.delivered.map(|d| d.mem_mib);
    let Some(needed) = needed_to_resume(store.resume_memory_mib(row.id)?, guest) else {
        return Ok(false);
    };
    let mut held = Some(0_u64);
    for id in store.ready_on(&node.name)? {
        let of_box = store.box_host_memory(id)?;
        held = held.zip(of_box).map(|(sum, mib)| sum.saturating_add(mib));
    }
    Ok(memory.beyond_reach(needed, held))
}

/// Before an asleep Box resumes: why it may not yet, after suspending an idle Box on its node to
/// make room when it is next; `None` when it may wake now.
pub(crate) fn room_to_wake(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    turn: Turn,
) -> Result<Option<String>, CommandError> {
    let Some(node) = recorded(cx, store, row)? else {
        return Ok(None);
    };
    let Some(memory) = node.memory else {
        return Ok(None);
    };
    if turn != Turn::Reached {
        let ahead = ahead_of(store, &node, row)?;
        if ahead > 0 {
            return Ok(Some(format!(
                "{} is making room; {ahead} Boxes ahead",
                node.name
            )));
        }
    }
    let Some(short) = resume_shortfall(store, &node, &memory, row, now(cx))? else {
        return Ok(None);
    };
    if turn != Turn::OnlyIfRoom && !beyond_reach(store, &node, &memory, row)? {
        suspend_idlest(cx, store, &node, short, row.name.as_str())?;
    }
    Ok(Some(short.reason(&node.name)))
}

/// What the head of the queue on `node` asks of it, passing over Boxes beyond reach: a resume what
/// it needs, a new start room to boot. Nothing when its Box count is full, since a suspension would
/// not help.
pub(super) fn demand(
    store: &Store,
    node: &Node,
    memory: &Memory,
    now: i64,
) -> Result<Demand, CommandError> {
    let mut head = None;
    for id in store.pending_on(&node.name)? {
        let Some(row) = store.box_row(id)? else {
            continue;
        };
        if !beyond_reach(store, node, memory, &row)? {
            head = Some(row);
            break;
        }
    }
    let Some(head) = head else {
        return Ok(Demand::default());
    };
    let asleep = head
        .boxed
        .as_ref()
        .is_some_and(|b| b.state == BoxState::Asleep);
    let short = if asleep {
        resume_shortfall(store, node, memory, &head, now)?
    } else {
        memory.shortfall(now)
    };
    let short = short.is_some_and(|s| helps(node, s));
    Ok(Demand {
        short,
        waiter: short.then(|| head.name.as_str().to_owned()),
    })
}
