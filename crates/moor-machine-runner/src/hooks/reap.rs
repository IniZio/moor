//! Clean-up of Boxes the runner holds no record for: files left on a machine or the host by a
//! lost record, a crash between a copy and its record, or a record removed by hand.

use std::collections::{BTreeSet, HashSet};
use std::sync::PoisonError;
use tokio::time::Instant;

use moor_core::box_key::BoxKey;
use moor_core::request::Request;
use moor_core::view::user::MachineTargetsValue;
use moor_core::BoxId;

use moor_client::Client;

use super::{open_boxes, tail, Hooks};
use moor_client::link::ops::Hook;

use crate::caller::Reach;
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
    /// each machine the controller lists at most once per `reconcile` period; a pass skips when
    /// that list cannot be had, rather than sweep fewer machines as though they were all. The open Boxes and the records are
    /// read after the listing: a Box that started meanwhile has files only once it is open.
    pub(super) async fn reconcile(&self, client: &Client) {
        let host = self.host_ids();
        let found = match self.machine_files(client).await {
            Ok(found) => found,
            Err(e) => return eprintln!("moor-machine-runner: reconcile skipped: {e}"),
        };
        let client = client.clone();
        let open = match tokio::task::spawn_blocking(move || open_boxes(&client)).await {
            Ok(Ok(open)) => open,
            Ok(Err(e)) => return eprintln!("moor-machine-runner: reconcile: {e}"),
            Err(e) => return eprintln!("moor-machine-runner: reconcile: {e}"),
        };
        let recorded: HashSet<i64> = match self.prepared() {
            Ok(ids) => ids.into_iter().collect(),
            Err(e) => return eprintln!("moor-machine-runner: cannot read the Box records: {e}"),
        };
        let orphan = |id: &i64| !open.contains(*id) && !recorded.contains(id);
        for id in host.iter().filter(|id| orphan(id)) {
            if let Err(e) = std::fs::remove_dir_all(self.mirrors.join(id.to_string())) {
                eprintln!("moor-machine-runner: cannot remove the mirror of Box {id}: {e}");
            }
        }
        for (target, ids) in &found {
            for id in ids.iter().filter(|id| orphan(id)) {
                if let Err(e) = self.release_orphan(target, *id).await {
                    eprintln!("moor-machine-runner: clean-up of Box {id} on {target} failed: {e}");
                }
            }
        }
    }

    /// The Box ids named on each machine the controller lists, when a listing is due; none
    /// otherwise. A machine that cannot be listed is logged and left for the next pass.
    ///
    /// # Errors
    /// Why the controller's list of machines could not be had.
    async fn machine_files(&self, client: &Client) -> Result<Vec<(String, BTreeSet<i64>)>, String> {
        let mut found = Vec::new();
        if !self.listing_due() {
            return Ok(found);
        }
        let client = client.clone();
        let targets = tokio::task::spawn_blocking(move || machine_targets(&client))
            .await
            .map_err(|e| e.to_string())??;
        for target in targets {
            match self.listing(&target).await {
                Ok(ids) => found.push((target, ids)),
                Err(e) => eprintln!("moor-machine-runner: cannot list the files on {target}: {e}"),
            }
        }
        Ok(found)
    }

    /// Whether the machines' files are due for a listing; marks it done when they are.
    fn listing_due(&self) -> bool {
        let mut last = self.listed.lock().unwrap_or_else(PoisonError::into_inner);
        if last.is_some_and(|at| at.elapsed() < self.reconcile) {
            return false;
        }
        *last = Some(Instant::now());
        true
    }

    /// The Box ids the host mirrors are named for.
    fn host_ids(&self) -> BTreeSet<i64> {
        std::fs::read_dir(&self.mirrors)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str().and_then(box_id))
            .collect()
    }

    /// The ids named by the entries under `~/moor` and `~/moor-state` on the machine.
    async fn listing(&self, target: &str) -> Result<BTreeSet<i64>, String> {
        // No glob: the machine's login shell may be zsh, which fails on a pattern that matches
        // nothing.
        let script = "cd && find moor moor-state -mindepth 1 -maxdepth 1 2>/dev/null; :";
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
        let reach = Reach::Ssh {
            ssh: target.to_owned(),
        };
        let env = [("MOOR_BOX".to_owned(), id.to_string())];
        self.hook(id, &reach, Hook::Release, &env).await?;
        self.remove_on_machine(&reach, id).await
    }

    /// Removes Box `id`'s copy and state dir on the machine.
    pub(super) async fn remove_on_machine(&self, reach: &Reach, id: i64) -> Result<(), String> {
        let key = BoxKey::new(BoxId::new(id));
        let (copy, state) = (key.machine_dir(), key.machine_state_dir());
        let script = format!("cd && rm -rf -- {copy} {state}");
        let out = self.helper(id, reach, &script).await?;
        match out.exit_code {
            Some(0) => Ok(()),
            _ => Err(format!(
                "could not remove {copy} and {state}: {}",
                tail(&out)
            )),
        }
    }
}

/// The ssh target of every machine reached over ssh, whichever user it belongs to. A machine
/// whose agent links in has none and is skipped: its orphans wait for clean-up over the link.
fn machine_targets(client: &Client) -> Result<BTreeSet<String>, String> {
    let response = client
        .request(Request::MachineTargets {}, |_| {})
        .map_err(|e| format!("the controller did not list the machines: {e}"))?;
    if let Some(error) = response.error {
        return Err(format!(
            "the controller refused to list the machines: {}",
            error.message
        ));
    }
    let value = response.value.unwrap_or_default();
    let listed: MachineTargetsValue = serde_json::from_value(value)
        .map_err(|e| format!("the controller's machine list is malformed: {e}"))?;
    Ok(listed
        .targets
        .into_iter()
        .map(|t| t.ssh)
        .filter(|t| !t.is_empty())
        .collect())
}
