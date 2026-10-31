//! Sweeping the folders closed Boxes leave on linked machines, over each agent's link.
//!
//! Every sweep period, and right after an agent links, each ordinary machine is told which Boxes
//! the controller lists as open and removes the folders of all others. That listing is the only
//! thing standing between a sweep and an open Box's files, so a listing that fails sends nothing.
//! A Box the runner holds a record for is also kept: its own release runs the hook with the
//! Box's facts, which a sweep does not have.

use std::sync::Arc;
use std::time::Duration;

use futures_util::future::join_all;
use moor_client::link::ops::{self, Op};
use moor_client::link::sweep::Swept;
use moor_client::Client;

use crate::hooks::{open_boxes, Hooks};
use crate::link::{self, Answer};
use crate::Agents;

/// Longest one machine's sweep may take, release hooks included.
const LIMIT: Duration = Duration::from_secs(ops::MAX_TIMEOUT_SECS);

/// Sweeps the linked machines every `every` and whenever an agent links.
pub(crate) fn spawn(every: Duration, client: &Client, agents: Option<Agents>, hooks: &Arc<Hooks>) {
    let Some(agents) = agents else {
        return;
    };
    let (client, hooks) = (client.clone(), Arc::clone(hooks));
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                () = agents.linked() => {}
            }
            sweep(&client, &agents, &hooks).await;
        }
    });
}

/// Sends every online machine the open Boxes, unless the controller will not list them.
async fn sweep(client: &Client, agents: &Agents, hooks: &Hooks) {
    let machines = agents.online();
    if machines.is_empty() {
        return;
    }
    let asks = client.clone();
    let open = match tokio::task::spawn_blocking(move || open_boxes(&asks)).await {
        Ok(Ok(open)) => open,
        Ok(Err(e)) => return eprintln!("moor-machine-runner: no machine sweep: {e}"),
        Err(e) => return eprintln!("moor-machine-runner: no machine sweep: {e}"),
    };
    let recorded = match hooks.prepared() {
        Ok(ids) => ids,
        Err(e) => return eprintln!("moor-machine-runner: no machine sweep: {e}"),
    };
    let mut keep: Vec<i64> = open.all.into_iter().chain(recorded).collect();
    keep.sort_unstable();
    keep.dedup();
    join_all(machines.into_iter().map(|id| machine(agents, id, &keep))).await;
}

/// Sweeps machine `id` and says what came of it.
async fn machine(agents: &Agents, id: i64, keep: &[i64]) {
    match tokio::time::timeout(LIMIT, ask(agents, id, keep)).await {
        Ok(Ok(swept)) => {
            for failed in &swept.errors {
                eprintln!(
                    "moor-machine-runner: machine {id} kept Box {}'s folders: {}",
                    failed.box_id, failed.reason
                );
            }
            if !swept.removed.is_empty() {
                eprintln!(
                    "moor-machine-runner: machine {id} removed the folders of Boxes {:?}",
                    swept.removed
                );
            }
        }
        Ok(Err(e)) => eprintln!("moor-machine-runner: machine {id} not swept: {e}"),
        Err(_) => eprintln!("moor-machine-runner: machine {id} not swept: timed out"),
    }
}

/// What machine `id` removed; a pool host removes nothing.
async fn ask(agents: &Agents, id: i64, keep: &[i64]) -> Result<Swept, String> {
    let sweep = Op::Sweep {
        keep: keep.to_vec(),
    };
    match link::send(Some(agents), &id.to_string(), id, &sweep).await? {
        Answer::Swept(swept) => Ok(swept),
        _ => Err("it answered no sweep".to_owned()),
    }
}
