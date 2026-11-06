//! Per-Box setup and clean-up on the machine: operator commands the runner runs over ssh.
//!
//! `prepare` runs before a Box's first `exec` or `machine_info` and prints the Box's facts,
//! which the runner keeps as the Box's record in the controller's store, so another runner that
//! takes the machine's link reads them; a `sync` leaves a record too. A sweep takes each Box on a
//! linked machine with a record that the controller no longer lists as open: it runs
//! `release` if the Box was prepared, removes the Box's copy on the machine, then drops its host
//! mirror and the record. A failed step keeps everything, so the next call or sweep retries.
//!
//! A Box the controller lists as asleep is suspended: its relay sessions and holds end and
//! `suspend` runs, and the record notes when. Its next call prepares it again, as does a call in
//! another slot. When a machine runs short of disk, suspended Boxes' files go, oldest first.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use moor_client::Client;
use moor_core::request::{BoxStage, Request, ShowReq};

use moor_client::link::ops::Hook;

use crate::caller::{Caller, Reach};
use crate::exec::ExecOutput;
use crate::holds::Holds;
use crate::relay::Relays;

mod reach;
mod reap;
mod records;
mod sleep;

/// What `prepare` printed about a Box, such as `device`.
pub(crate) type Facts = BTreeMap<String, String>;

/// Lines of a failed command's output quoted in its error.
const TAIL_LINES: usize = 20;

/// The operator's commands and where the runner keeps what they leave.
pub(crate) struct Hooks {
    /// Free GB a machine must have before a Box is prepared or synced there; 0 disables.
    pub(crate) min_free_gb: u64,
    /// Client for the controller's operator socket, to list the Boxes asleep and the machines.
    pub(crate) client: Client,
    /// How long either may run.
    pub(crate) timeout: Duration,
    /// Parent of the per-Box host mirrors, which release removes.
    pub(crate) mirrors: PathBuf,
    /// One lock per Box id, so two calls of a Box never prepare it twice.
    pub(crate) preparing: Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>,
    /// The links that carry hooks for enrolled machines, whose own hooks then run by name.
    pub(crate) agents: Option<crate::Agents>,
}

/// What the runner keeps about a Box it set up: where it is, for `release`, and its facts.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Record {
    /// How the runner reaches the machine.
    reach: Reach,
    /// The Box's slot on the machine.
    slot: u16,
    /// What `prepare` printed.
    facts: Facts,
    /// Whether `prepare` ran, and `suspend` since.
    stage: BoxStage,
    /// Whether the Box's files on the machine were removed while it slept; until its next sync.
    evicted: bool,
}

impl Record {
    /// Whether the facts still hold for `caller`: prepared, awake, and in the same slot.
    fn is_current(&self, caller: &Caller) -> bool {
        self.stage == BoxStage::Prepared && self.slot == caller.slot
    }

    /// A record of a Box at `stage` whose files were not evicted.
    fn new(caller: &Caller, facts: Facts, stage: BoxStage) -> Self {
        Self {
            reach: caller.reach.clone(),
            slot: caller.slot,
            facts,
            stage,
            evicted: false,
        }
    }
}

impl Hooks {
    /// The caller's facts, running `prepare` first if the Box has no record yet, was suspended,
    /// or is in another slot than the record says.
    pub(crate) async fn facts(&self, caller: &Caller) -> Result<Facts, String> {
        let id = caller.box_id.get();
        let lock = self.lock_for(id);
        let _held = lock.lock().await;
        let old = self.load(id).await?;
        if let Some(record) = old.as_ref().filter(|r| r.is_current(caller)) {
            return Ok(record.facts.clone());
        }
        self.guard(caller).await?;
        let env = box_env(id, caller.slot, &Facts::new());
        let out = self.hook(id, &caller.reach, Hook::Prepare, &env);
        let facts = match out.await.map_err(|e| format!("prepare: {e}"))? {
            None => Facts::new(),
            Some(out) => {
                let last = out.output.lines().rev().find(|l| !l.trim().is_empty());
                serde_json::from_str(last.unwrap_or_default()).map_err(|e| {
                    format!(
                        "prepare's last line is not a JSON object of strings ({e}): {}",
                        tail(&out)
                    )
                })?
            }
        };
        let mut record = Record::new(caller, facts, BoxStage::Prepared);
        record.evicted = old.is_some_and(|r| r.evicted);
        self.keep(id, &record)
            .await
            .map_err(|e| format!("could not keep the Box's facts: {e}"))?;
        Ok(record.facts)
    }

    /// Leaves a record for the caller's Box when it has none, so a sweep finds it once closed.
    pub(crate) async fn note(&self, caller: &Caller) -> Result<(), String> {
        let id = caller.box_id.get();
        let lock = self.lock_for(id);
        let _held = lock.lock().await;
        if self.load(id).await?.is_some() {
            return Ok(());
        }
        let record = Record::new(caller, Facts::new(), BoxStage::Noted);
        self.keep(id, &record)
            .await
            .map_err(|e| format!("could not note the Box: {e}"))
    }

    /// The lock that keeps two calls of Box `id` from preparing it twice.
    fn lock_for(&self, id: i64) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .preparing
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Arc::clone(locks.entry(id).or_default())
    }

    /// Releases every Box on a linked machine with a record that the controller does not list as
    /// open, ending its relay sessions first, suspends the open ones that sleep, then cleans up
    /// Boxes it has no record for.
    pub(crate) async fn sweep(&self, client: &Client, relays: &Relays, holds: &Holds) {
        let prepared = match self.recorded().await {
            Ok(ids) => ids,
            Err(e) => return eprintln!("moor-machine-runner: cannot read the Box records: {e}"),
        };
        let asks = client.clone();
        let open = match tokio::task::spawn_blocking(move || open_boxes(&asks)).await {
            Ok(Ok(open)) => open,
            Ok(Err(e)) => return eprintln!("moor-machine-runner: sweep: {e}"),
            Err(e) => return eprintln!("moor-machine-runner: sweep: {e}"),
        };
        for id in prepared.iter().filter(|id| open.asleep.contains(id)) {
            if let Err(e) = self.suspend(*id, relays, holds).await {
                eprintln!("moor-machine-runner: suspend of Box {id} failed, retrying: {e}");
            }
        }
        for id in prepared.into_iter().filter(|id| !open.contains(*id)) {
            if let Err(e) = self.release(id, relays).await {
                eprintln!("moor-machine-runner: release of Box {id} failed, retrying: {e}");
            }
        }
        self.reconcile(client).await;
    }

    /// Ends Box `id`'s relay sessions, runs `release`, then drops its host mirror and its record.
    async fn release(&self, id: i64, relays: &Relays) -> Result<(), String> {
        let lock = self.lock_for(id);
        let _held = lock.lock().await;
        relays.end_box(id).await;
        let Some(record) = self.load(id).await? else {
            return Ok(());
        };
        self.release_files(id, &record).await?;
        match std::fs::remove_dir_all(self.mirrors.join(id.to_string())) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.to_string()),
            _ => {}
        }
        self.forget(id).await
    }

    /// Runs `release` for a prepared Box, then removes its copy and state dir on the machine.
    async fn release_files(&self, id: i64, record: &Record) -> Result<(), String> {
        if record.stage != BoxStage::Noted {
            let env = box_env(id, record.slot, &record.facts);
            self.hook(id, &record.reach, Hook::Release, &env).await?;
        }
        self.remove_on_machine(&record.reach, id).await
    }
}

/// What a command run for Box `id` in `slot` sees: its id, slot, port offset and facts.
pub(crate) fn box_env(id: i64, slot: u16, facts: &Facts) -> Vec<(String, String)> {
    let mut env = vec![
        ("MOOR_BOX".to_owned(), id.to_string()),
        ("MOOR_SLOT".to_owned(), slot.to_string()),
        (
            "MOOR_PORT_OFFSET".to_owned(),
            (10 * u32::from(slot)).to_string(),
        ),
    ];
    env.extend(fact_env(facts));
    env
}

/// `facts` as env: `MOOR_FACT_<KEY>`, the key uppercased with every other character as `_`.
pub(crate) fn fact_env(facts: &Facts) -> Vec<(String, String)> {
    facts
        .iter()
        .map(|(k, v)| {
            let key: String = k
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_uppercase()
                    } else {
                        '_'
                    }
                })
                .collect();
            (format!("MOOR_FACT_{key}"), v.clone())
        })
        .collect()
}

/// Every `every`, ends idle relay sessions and sweeps the Boxes that are no longer open.
pub(crate) fn spawn_sweep(
    every: Duration,
    client: Client,
    hooks: Arc<Hooks>,
    relays: Arc<Relays>,
    holds: Arc<Holds>,
) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            relays.expire().await;
            hooks.sweep(&client, &relays, &holds).await;
        }
    });
}

/// The Boxes the controller lists as open, and which of them sleep.
#[derive(Default)]
pub(super) struct OpenBoxes {
    /// Every open Box.
    pub(super) all: HashSet<i64>,
    /// The open Boxes that are asleep.
    pub(super) asleep: HashSet<i64>,
}

impl OpenBoxes {
    /// Whether the controller lists Box `id` as open.
    pub(super) fn contains(&self, id: i64) -> bool {
        self.all.contains(&id)
    }
}

/// The Boxes the controller lists as open.
pub(super) fn open_boxes(client: &Client) -> Result<OpenBoxes, String> {
    let request = Request::Show(ShowReq {
        target: None,
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    });
    let response = client
        .request(request, |_| {})
        .map_err(|e| format!("the controller did not list the open Boxes: {e}"))?;
    if let Some(error) = response.error {
        return Err(format!(
            "the controller refused to list the open Boxes: {}",
            error.message
        ));
    }
    let boxes = response
        .value
        .as_ref()
        .and_then(|v| v.get("boxes"))
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the controller's Box list has no ids".to_owned())?;
    let mut open = OpenBoxes::default();
    for b in boxes {
        let id = b
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| "the controller's Box list has no ids".to_owned())?;
        open.all.insert(id);
        if b.get("lifecycle").and_then(serde_json::Value::as_str) == Some("asleep") {
            open.asleep.insert(id);
        }
    }
    Ok(open)
}

/// `out` when it exited 0, else an error quoting its tail.
fn succeeded(out: ExecOutput) -> Result<ExecOutput, String> {
    match out.exit_code {
        Some(0) => Ok(out),
        _ if out.timed_out => Err(format!("timed out: {}", tail(&out))),
        code => Err(format!("exit {code:?}: {}", tail(&out))),
    }
}

/// The last [`TAIL_LINES`] lines of a command's output.
fn tail(out: &ExecOutput) -> String {
    let lines: Vec<&str> = out.output.lines().collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}
