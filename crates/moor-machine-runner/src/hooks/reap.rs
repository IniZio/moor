//! Clean-up of Boxes the runner holds no record for: files left on a machine or the host by a
//! lost record, a crash between a copy and its record, or a record removed by hand.

use std::collections::{BTreeSet, HashSet};
use std::io::{self, Write};
use std::sync::PoisonError;
use tokio::time::Instant;

use moor_core::box_key::BoxKey;
use moor_core::BoxId;

use super::{read, tail, Hooks};
use crate::exec;

/// Shell lines that give a command a state dir of its own and a `TMPDIR` inside it, so what the
/// Box's tools write there goes with the Box.
pub(crate) fn state_prelude(id: i64) -> String {
    let dir = BoxKey::new(BoxId::new(id)).machine_state_dir();
    format!(
        "export MOOR_STATE_DIR=\"$HOME/{dir}\"; export TMPDIR=\"$MOOR_STATE_DIR/tmp\"; \
         mkdir -p \"$TMPDIR\" || exit 1; "
    )
}

/// The Box id a directory is named for, or none for any other name.
fn box_id(name: &str) -> Option<i64> {
    let id: i64 = name.parse().ok().filter(|id| *id > 0)?;
    (id.to_string() == name).then_some(id)
}

impl Hooks {
    /// Cleans up every closed Box that has no record: its host mirror at once, and its files on
    /// each known machine at most once per `reconcile` period.
    pub(super) async fn reconcile(&self, open: &HashSet<i64>) {
        let recorded: HashSet<i64> = match self.prepared() {
            Ok(ids) => ids.into_iter().collect(),
            Err(e) => return eprintln!("moor-machine-runner: cannot read the Box records: {e}"),
        };
        self.reap_host(open, &recorded);
        {
            let mut last = self.listed.lock().unwrap_or_else(PoisonError::into_inner);
            if last.is_some_and(|at| at.elapsed() < self.reconcile) {
                return;
            }
            *last = Some(Instant::now());
        }
        for target in self.targets() {
            let ids = match self.listing(&target).await {
                Ok(ids) => ids,
                Err(e) => {
                    eprintln!("moor-machine-runner: cannot list the files on {target}: {e}");
                    continue;
                }
            };
            for id in ids
                .iter()
                .filter(|id| !recorded.contains(id) && !open.contains(id))
            {
                if let Err(e) = self.release_orphan(&target, *id).await {
                    eprintln!("moor-machine-runner: clean-up of Box {id} on {target} failed: {e}");
                }
            }
        }
    }

    /// Removes the host mirror of each closed Box that has no record.
    fn reap_host(&self, open: &HashSet<i64>, recorded: &HashSet<i64>) {
        let Ok(entries) = std::fs::read_dir(&self.mirrors) else {
            return;
        };
        for entry in entries.flatten() {
            let id = entry.file_name().to_str().and_then(box_id);
            let Some(id) = id.filter(|id| !open.contains(id) && !recorded.contains(id)) else {
                continue;
            };
            if let Err(e) = std::fs::remove_dir_all(self.mirrors.join(id.to_string())) {
                eprintln!("moor-machine-runner: cannot remove the mirror of Box {id}: {e}");
            }
        }
    }

    /// The ids named by the entries under `~/moor` and `~/moor-state` on the machine.
    async fn listing(&self, target: &str) -> Result<BTreeSet<i64>, String> {
        let script = "cd && for e in moor/* moor-state/*; do [ -e \"$e\" ] && echo \"$e\"; done; :";
        let out = exec::run(target, script, self.timeout)
            .await
            .map_err(|e| format!("could not run the remote shell: {e}"))?;
        if out.exit_code != Some(0) {
            return Err(tail(&out));
        }
        Ok(out
            .output
            .lines()
            .map(str::trim)
            .filter_map(|l| {
                l.strip_prefix("moor-state/")
                    .or_else(|| l.strip_prefix("moor/"))
            })
            .filter_map(box_id)
            .collect())
    }

    /// Runs `release` for a Box without a record, so without facts, then removes its files.
    async fn release_orphan(&self, target: &str, id: i64) -> Result<(), String> {
        if let Some(command) = &self.release {
            let env = [("MOOR_BOX".to_owned(), id.to_string())];
            self.on_machine(id, target, &env, command).await?;
        }
        self.remove_on_machine(target, id).await
    }

    /// Removes Box `id`'s copy and state dir on the machine.
    pub(super) async fn remove_on_machine(&self, target: &str, id: i64) -> Result<(), String> {
        let key = BoxKey::new(BoxId::new(id));
        let (copy, state) = (key.machine_dir(), key.machine_state_dir());
        let script = format!("cd && rm -rf -- {copy} {state}");
        let out = exec::run(target, &script, self.timeout)
            .await
            .map_err(|e| format!("could not run the remote shell: {e}"))?;
        match out.exit_code {
            Some(0) => Ok(()),
            _ => Err(format!(
                "could not remove {copy} and {state}: {}",
                tail(&out)
            )),
        }
    }

    /// Every machine the runner has served or holds a record for.
    fn targets(&self) -> BTreeSet<String> {
        let mut all: BTreeSet<String> = std::fs::read_to_string(&self.machines)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect();
        for id in self.prepared().unwrap_or_default() {
            if let Ok(Some(record)) = read(&self.record(id)) {
                all.insert(record.ssh);
            }
        }
        all
    }

    /// Keeps `target` among the machines to look through, past the loss of every record.
    pub(super) fn remember(&self, target: &str) -> io::Result<()> {
        if self.targets().contains(target) {
            return Ok(());
        }
        if let Some(dir) = self.machines.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.machines)?;
        writeln!(file, "{target}")
    }
}
