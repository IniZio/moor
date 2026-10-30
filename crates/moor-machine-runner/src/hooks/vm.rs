//! A Box's VM on a VM host. crabbox and its provider run on the host, so the runner drives them
//! over the host's ssh, from a lease dir under the Box's state dir there. The VM's slug is fixed
//! per Box, so a VM a crashed boot left behind is found by name and deleted.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use moor_core::box_key::BoxKey;
use moor_core::ssh::pin::{self, KnownHosts};
use moor_core::ssh::split_words;
use moor_core::BoxId;
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use super::{read, tail, write, Facts, Hooks, Record};
use crate::beat::Progress;
use crate::caller::{Caller, Reach};
use crate::exec::{quote, ExecOutput};

/// How often a boot reports how long it has run.
const BOOT_BEAT: Duration = Duration::from_secs(10);

/// The Box's VM as its record keeps it: planned before the boot, with its words once ready.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Vm {
    /// The crabbox provider on the host.
    provider: String,
    /// The lease's slug, which is also its id.
    slug: String,
    /// ssh words from the host into the VM, kept only after they passed the pin check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    words: Option<Vec<String>>,
    /// Whether `crabbox pause` succeeded while the Box slept, so its wake resumes it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) paused: bool,
}

impl Vm {
    /// The VM crabbox names `moor-<id>` with `provider`, not yet booted.
    pub(super) fn planned(provider: &str, id: i64) -> Self {
        Self {
            provider: provider.to_owned(),
            slug: format!("moor-{id}"),
            words: None,
            paused: false,
        }
    }

    /// `crabbox <verb> --provider P <name flag> S`, then `more`.
    fn args(&self, verb: &str, name: &str, more: &[&str]) -> Vec<String> {
        let mut args = vec![verb, "--provider", &self.provider, name, &self.slug];
        args.extend(more);
        args.into_iter().map(str::to_owned).collect()
    }
}

impl Hooks {
    /// The ssh words into the caller's VM, booting it first when its record keeps none; whether
    /// it booted. A boot that stopped part way leaves a planned VM, which is deleted first.
    pub(super) async fn boot(
        &self,
        caller: &Caller,
        provider: &str,
        progress: Option<&Progress>,
    ) -> Result<(Vec<String>, bool), String> {
        let id = caller.box_id.get();
        let path = self.record(id);
        let mut old = read(&path)?;
        if let Some(record) = old.take_if(|r| r.vm.as_ref().is_some_and(|v| v.paused)) {
            return self.wake(caller, &path, record).await.map(|w| (w, false));
        }
        let planned = match old.as_ref().and_then(|r| r.vm.as_ref()) {
            Some(Vm {
                words: Some(words), ..
            }) => return Ok((words.clone(), false)),
            Some(vm) => Some(vm.clone()),
            None => None,
        };
        if !matches!(caller.reach, Reach::Ssh { .. }) {
            return Err(format!(
                "VM host {} is not reached over ssh",
                caller.machine
            ));
        }
        if let Some(vm) = &planned {
            self.stop(&caller.reach, id, vm)
                .await
                .map_err(|e| format!("could not delete the half-booted {}: {e}", vm.slug))?;
        }
        let vm = Vm::planned(provider, id);
        let mut record = old.unwrap_or_else(|| Record::new(caller, Facts::new(), false));
        record.prepared = false;
        record.vm = Some(vm.clone());
        write(&path, &record).map_err(|e| format!("could not note the VM: {e}"))?;
        let what = format!("booting {} on {}", vm.slug, caller.machine);
        let warm = vm.args("warmup", "--slug", &[]);
        let warmup = self.crabbox(&caller.reach, id, &warm);
        tokio::select! {
            done = warmup => done?,
            () = beat(progress, &what) => unreachable!("a beat never ends"),
        };
        let words = self.checked_words(caller, &path, record, vm).await?;
        Ok((words, true))
    }

    /// Resumes the caller's paused VM, then reads and checks its words again, since a resumed
    /// VM may answer with other ssh options.
    async fn wake(
        &self,
        caller: &Caller,
        path: &Path,
        record: Record,
    ) -> Result<Vec<String>, String> {
        let id = caller.box_id.get();
        let vm = record.vm.clone().ok_or("the Box has no VM to resume")?;
        self.crabbox(&caller.reach, id, &vm.args("resume", "--id", &[]))
            .await
            .map_err(|e| format!("could not resume {}: {e}", vm.slug))?;
        let vm = Vm {
            words: None,
            paused: false,
            ..vm
        };
        self.checked_words(caller, path, record, vm).await
    }

    /// The words `crabbox ssh` prints for `vm`, kept in the record once they pass the pin
    /// check; a VM whose words fail it is deleted.
    async fn checked_words(
        &self,
        caller: &Caller,
        path: &Path,
        mut record: Record,
        vm: Vm,
    ) -> Result<Vec<String>, String> {
        let id = caller.box_id.get();
        let printed = self
            .crabbox(&caller.reach, id, &vm.args("ssh", "--id", &[]))
            .await?;
        let words = ssh_words(&printed)?;
        if let Err(why) = self.pinned(&caller.reach, &words).await {
            let refusal = pin::refusal(&why);
            self.stop(&caller.reach, id, &vm)
                .await
                .map_err(|e| format!("{refusal}; deleting the VM failed too: {e}"))?;
            record.vm = None;
            write(path, &record).map_err(|e| format!("{refusal}; {e}"))?;
            return Err(refusal);
        }
        record.vm = Some(Vm {
            words: Some(words.clone()),
            ..vm
        });
        write(path, &record).map_err(|e| format!("could not keep the VM's words: {e}"))?;
        Ok(words)
    }

    /// Pauses the Box's VM; whether it paused. A provider that cannot pause, or a pause that
    /// fails, leaves the VM running, which only costs the host its resources.
    pub(super) async fn pause(&self, reach: &Reach, id: i64, vm: &Vm) -> bool {
        match self
            .crabbox(reach, id, &vm.args("pause", "--id", &[]))
            .await
        {
            Ok(_) => true,
            Err(e) if e.contains("does not support pause") => {
                eprintln!(
                    "moor-machine-runner: provider {} cannot pause {}; it keeps running",
                    vm.provider, vm.slug
                );
                false
            }
            Err(e) => {
                eprintln!("moor-machine-runner: {} keeps running: {e}", vm.slug);
                false
            }
        }
    }

    /// The Box ids of the `moor-<id>` leases `crabbox list` shows for `provider` on `reach`.
    pub(super) async fn leases(
        &self,
        reach: &Reach,
        provider: &str,
    ) -> Result<std::collections::BTreeSet<i64>, String> {
        let script = format!("crabbox list --provider {} --json", quote(provider));
        let out = self.helper(0, reach, &script).await?;
        if out.exit_code != Some(0) {
            return Err(format!("crabbox list failed: {}", tail(&out)));
        }
        lease_ids(&out.output)
    }

    /// Deletes the Box's VM on the host; a VM already gone counts as deleted.
    pub(super) async fn stop(&self, reach: &Reach, id: i64, vm: &Vm) -> Result<(), String> {
        let args = vm.args("stop", "--id", &[]);
        self.crabbox_exiting(reach, id, &args, &[0, GONE])
            .await
            .map(drop)
    }

    /// Runs crabbox with `args` on the host, from Box `id`'s lease dir; its output when it
    /// exits 0, else an error quoting the tail.
    async fn crabbox(&self, reach: &Reach, id: i64, args: &[String]) -> Result<ExecOutput, String> {
        self.crabbox_exiting(reach, id, args, &[0]).await
    }

    /// Runs crabbox like [`Self::crabbox`], taking any exit code in `ok` as success.
    async fn crabbox_exiting(
        &self,
        reach: &Reach,
        id: i64,
        args: &[String],
        ok: &[i32],
    ) -> Result<ExecOutput, String> {
        let state = BoxKey::new(BoxId::new(id)).machine_state_dir();
        let words: Vec<String> = args.iter().map(|a| quote(a)).collect();
        let script = format!(
            "mkdir -p ~/{state}/lease && cd ~/{state}/lease && \
             GIT_CEILING_DIRECTORIES=\"$HOME/{state}\" crabbox {}",
            words.join(" ")
        );
        let out = self.helper(id, reach, &script).await?;
        match out.exit_code {
            Some(code) if ok.contains(&code) => Ok(out),
            code => Err(format!(
                "crabbox {} failed ({code:?}): {}",
                args[0],
                tail(&out)
            )),
        }
    }

    /// Ok when `words` pin the VM's host key, by `ssh -G` and the known-hosts files on the host.
    async fn pinned(&self, reach: &Reach, words: &[String]) -> Result<(), String> {
        let words: Vec<String> = words.iter().map(|w| quote(w)).collect();
        let script = format!(
            "o=$(ssh -G {}) || exit 1\nprintf '%s\\n' \"$o\"\n{PROBE}",
            words.join(" ")
        );
        let out = self.helper(0, reach, &script).await?;
        if out.exit_code != Some(0) {
            return Err("ssh -G could not read them".to_owned());
        }
        let files = Probed(
            out.output
                .lines()
                .filter_map(|l| l.strip_prefix("moor-known-hosts "))
                .filter_map(|l| l.split_once(' '))
                .map(|(state, file)| (file.to_owned(), state.to_owned()))
                .collect(),
        );
        pin::check(&out.output, &files)
    }
}

/// The ssh words, without `ssh`, in what `crabbox ssh` printed.
fn ssh_words(printed: &ExecOutput) -> Result<Vec<String>, String> {
    printed
        .output
        .lines()
        .filter_map(split_words)
        .find(|w| w.first().is_some_and(|w| w == "ssh"))
        .map(|w| w[1..].to_vec())
        .ok_or_else(|| format!("crabbox ssh printed no ssh line: {}", tail(printed)))
}

/// The Box ids in `crabbox list --json` output: leases whose slug is `moor-<id>`. A lease
/// names its slug in `labels.slug`, `slug` or `name`, by provider.
fn lease_ids(json: &str) -> Result<std::collections::BTreeSet<i64>, String> {
    let leases: Vec<serde_json::Value> = serde_json::from_str(json.trim())
        .map_err(|e| format!("crabbox list printed no JSON list: {e}"))?;
    Ok(leases
        .iter()
        .filter_map(|l| {
            [&l["labels"]["slug"], &l["slug"], &l["name"]]
                .into_iter()
                .find_map(serde_json::Value::as_str)
        })
        .filter_map(|slug| slug.strip_prefix("moor-"))
        .filter_map(super::reap::box_id)
        .collect())
}

/// The exit code of `crabbox stop` for a lease that is already gone.
const GONE: i32 = 4;

/// Prints `moor-known-hosts dev|w|ro FILE` for each known-hosts file in `$o`.
const PROBE: &str = r#"for f in $(printf '%s\n' "$o" | sed -n 's/^userknownhostsfile //p'); do
  if [ -c "$f" ]; then s=dev
  elif [ -e "$f" ]; then if [ -w "$f" ]; then s=w; else s=ro; fi
  elif [ -w "$(dirname "$f")" ]; then s=w; else s=ro; fi
  printf 'moor-known-hosts %s %s\n' "$s" "$f"
done"#;

/// The known-hosts files on the host, as [`PROBE`] found them.
struct Probed(HashMap<String, String>);

impl KnownHosts for Probed {
    fn is_device(&self, file: &str) -> bool {
        self.0.get(file).is_some_and(|s| s == "dev")
    }

    fn can_add_to(&self, file: &str) -> bool {
        self.0.get(file).is_some_and(|s| s == "w")
    }
}

/// Notes `what` and how long it has run, at once and every [`BOOT_BEAT`], until dropped.
async fn beat(progress: Option<&Progress>, what: &str) {
    let Some(progress) = progress else {
        return std::future::pending().await;
    };
    let started = Instant::now();
    let mut tick = tokio::time::interval(BOOT_BEAT);
    loop {
        tick.tick().await;
        let secs = started.elapsed().as_secs();
        progress.note(format!("{what}: {secs} s")).await;
    }
}
