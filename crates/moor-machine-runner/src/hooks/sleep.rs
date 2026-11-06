//! Boxes that sleep and machines that run short of disk.
//!
//! A sleeping Box frees its slot, so its holds and relay sessions end and `suspend` runs; its
//! files stay. Before a Box is prepared or synced on a machine, the machine's free space is
//! checked against the floor, and the files of suspended Boxes go, oldest first, until it is met.

use std::time::{SystemTime, UNIX_EPOCH};

use super::{box_env, open_boxes, tail, Facts, Hooks, Record};
use moor_client::link::ops::Hook;
use moor_core::request::BoxStage;

use crate::caller::{Caller, Reach};
use crate::holds::Holds;
use crate::relay::Relays;

/// KiB in a GB.
const KIB_PER_GB: u64 = 1024 * 1024;

impl Hooks {
    /// Ends Box `id`'s relay sessions and holds and runs `suspend`, once: a Box that is not
    /// prepared or already suspended is left alone. The record notes the time only after the
    /// hook succeeded, so a failure is retried by the next sweep.
    pub(crate) async fn suspend(
        &self,
        id: i64,
        relays: &Relays,
        holds: &Holds,
    ) -> Result<(), String> {
        let lock = self.lock_for(id);
        let _held = lock.lock().await;
        let Some(mut record) = self
            .load(id)
            .await?
            .filter(|r| r.stage == BoxStage::Prepared)
        else {
            return Ok(());
        };
        relays.end_box(id).await;
        holds.release_box(id);
        let env = box_env(id, record.slot, &record.facts);
        self.hook(id, &record.reach, Hook::Suspend, &env).await?;
        record.stage = BoxStage::Suspended { at: now() };
        self.keep(id, &record)
            .await
            .map_err(|e| format!("could not note the suspend: {e}"))
    }

    /// Makes room for a call of `caller`'s Box on its machine: with less free space than the
    /// floor, removes the files of suspended Boxes there that are still asleep, oldest first,
    /// until the floor is met; an error naming the space when it cannot be.
    pub(super) async fn guard(&self, caller: &Caller) -> Result<(), String> {
        if self.min_free_gb == 0 {
            return Ok(());
        }
        let floor = self.min_free_gb.saturating_mul(KIB_PER_GB);
        let mut free = self.free_kib(caller).await?;
        if free >= floor {
            return Ok(());
        }
        for id in self.sleepers(&caller.reach, caller.box_id.get()).await {
            if let Err(e) = self.evict(id).await {
                eprintln!("moor-machine-runner: could not remove the files of Box {id}: {e}");
                continue;
            }
            free = self.free_kib(caller).await?;
            if free >= floor {
                return Ok(());
            }
        }
        let tenths = free.saturating_mul(10) / KIB_PER_GB;
        Err(format!(
            "{}.{} GB free on {}, below the {} GB floor, and no sleeping Box is left to evict",
            tenths / 10,
            tenths % 10,
            caller.machine,
            self.min_free_gb
        ))
    }

    /// Free KiB in the home of `caller`'s machine user.
    async fn free_kib(&self, caller: &Caller) -> Result<u64, String> {
        let out = self
            .helper(caller.box_id.get(), &caller.reach, "df -Pk ~")
            .await?;
        if out.exit_code != Some(0) {
            return Err(format!(
                "could not read the free disk space: {}",
                tail(&out)
            ));
        }
        parse_free_kib(&out.output).map_err(|e| format!("could not read the free disk space: {e}"))
    }

    /// The suspended Boxes on `target` that the controller still lists asleep, besides
    /// `except`, oldest sleep first.
    async fn sleepers(&self, target: &Reach, except: i64) -> Vec<i64> {
        let client = self.client.clone();
        let open = match tokio::task::spawn_blocking(move || open_boxes(&client)).await {
            Ok(Ok(open)) => open,
            Ok(Err(e)) => {
                eprintln!("moor-machine-runner: {e}");
                return Vec::new();
            }
            Err(e) => {
                eprintln!("moor-machine-runner: {e}");
                return Vec::new();
            }
        };
        let records = match self.on_machine(target.machine_id).await {
            Ok(records) => records,
            Err(e) => {
                eprintln!("moor-machine-runner: {e}");
                return Vec::new();
            }
        };
        let mut found: Vec<(u64, i64)> = records
            .into_iter()
            .filter(|(id, _)| *id != except && open.asleep.contains(id))
            .filter_map(|(id, record)| match record.stage {
                BoxStage::Suspended { at } => Some((at, id)),
                BoxStage::Noted | BoxStage::Prepared => None,
            })
            .collect();
        found.sort_unstable();
        found.into_iter().map(|(_, id)| id).collect()
    }

    /// Runs `release` for suspended Box `id` and removes its files, keeping its record as
    /// evicted. A Box in the middle of a call is skipped.
    async fn evict(&self, id: i64) -> Result<(), String> {
        let lock = self.lock_for(id);
        let _held = lock
            .try_lock()
            .map_err(|_| "it is being called".to_owned())?;
        let Some(record) = self
            .load(id)
            .await?
            .filter(|r| matches!(r.stage, BoxStage::Suspended { .. }))
        else {
            return Err("it is no longer suspended".to_owned());
        };
        self.release_files(id, &record).await?;
        let evicted = Record {
            facts: Facts::new(),
            stage: BoxStage::Noted,
            evicted: true,
            ..record
        };
        self.keep(id, &evicted)
            .await
            .map_err(|e| format!("could not note the eviction: {e}"))
    }

    /// Before a sync: makes room, and prepares a Box that was suspended or moved slot.
    pub(crate) async fn before_sync(&self, caller: &Caller) -> Result<(), String> {
        self.guard(caller).await?;
        self.note(caller).await?;
        let stale = self
            .load(caller.box_id.get())
            .await?
            .is_some_and(|r| r.stage != BoxStage::Noted && !r.is_current(caller));
        if stale {
            self.facts(caller).await?;
        }
        Ok(())
    }

    /// After a sync: the Box's files are back, so the notice of their removal ends.
    pub(crate) async fn synced(&self, caller: &Caller) -> Result<(), String> {
        let id = caller.box_id.get();
        let lock = self.lock_for(id);
        let _held = lock.lock().await;
        match self.load(id).await? {
            Some(record) if record.evicted => self
                .keep(
                    id,
                    &Record {
                        evicted: false,
                        ..record
                    },
                )
                .await
                .map_err(|e| format!("could not keep the Box's record: {e}")),
            _ => Ok(()),
        }
    }

    /// What a Box must know about its files, if they were removed while it slept.
    pub(crate) async fn notice(&self, caller: &Caller) -> Option<String> {
        let record = self.load(caller.box_id.get()).await.ok().flatten()?;
        record.evicted.then(|| {
            format!(
                "The files of this Box on {} were removed while it slept to free disk space. \
                 Run sync again before you use them.",
                caller.machine
            )
        })
    }
}

/// Free KiB in the output of `df -Pk <path>`: the Available column of the last line.
fn parse_free_kib(output: &str) -> Result<u64, String> {
    let line = output
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .ok_or("df printed nothing")?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 6 {
        return Err(format!("not a df -Pk line: {line}"));
    }
    fields[3]
        .parse()
        .map_err(|_| format!("not a df -Pk line: {line}"))
}

/// Seconds since the unix epoch.
#[allow(
    clippy::disallowed_methods,
    reason = "orders sleeping Boxes for eviction; the runner has no injected clock"
)]
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::parse_free_kib;

    #[test]
    fn reads_the_available_column_of_gnu_and_bsd_df() {
        let gnu = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n\
                   /dev/sda1        490691512 102400000 363000000      23% /\n";
        assert_eq!(parse_free_kib(gnu), Ok(363_000_000));
        let mac = "Filesystem   1024-blocks      Used Available Capacity  Mounted on\n\
                   /dev/disk3s5   971350180 600000000  12345678    98%    /System/Volumes/Data\n";
        assert_eq!(parse_free_kib(mac), Ok(12_345_678));
    }

    #[test]
    fn refuses_output_that_is_not_a_df_line() {
        assert!(parse_free_kib("").is_err());
        assert!(parse_free_kib("Filesystem 1024-blocks Used Available Capacity\n").is_err());
        assert!(parse_free_kib("df: no such file\n").is_err());
    }
}
