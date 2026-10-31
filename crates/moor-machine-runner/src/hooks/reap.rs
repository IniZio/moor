//! Clean-up of Boxes the runner holds no record for: files left on a machine or the host by a
//! lost record, a crash between a copy and its record, or a record removed by hand.

use std::collections::{BTreeSet, HashSet};

use moor_client::link::ops::Op;
use moor_client::Client;

use super::{open_boxes, Hooks};

use crate::caller::Reach;
use crate::link::{self, Answer};

/// The Box id a directory is named for, or none for any other name.
fn box_id(name: &str) -> Option<i64> {
    let id: i64 = name.parse().ok().filter(|id| *id > 0)?;
    (id.to_string() == name).then_some(id)
}

impl Hooks {
    /// Cleans up every closed Box that has no record: its host mirror. The open Boxes and the
    /// records are read first: a Box that started meanwhile has files only once it is open.
    pub(super) async fn reconcile(&self, client: &Client) {
        let host = self.host_ids();
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

    /// Removes Box `id`'s copy and state dir on the machine, over a link op every ordinary
    /// machine serves, `--allow-exec` or not.
    pub(super) async fn remove_on_machine(&self, reach: &Reach, id: i64) -> Result<(), String> {
        let Reach {
            machine_id,
            machine,
        } = reach;
        let op = Op::Remove { box_id: id };
        match link::send(self.agents.as_ref(), machine, *machine_id, &op).await? {
            Answer::Swept(swept) => match swept.errors.into_iter().next() {
                None => Ok(()),
                Some(failed) => Err(format!(
                    "could not remove the files of Box {id}: {}",
                    failed.reason
                )),
            },
            _ => Err(format!("machine {machine} answered no remove")),
        }
    }
}
