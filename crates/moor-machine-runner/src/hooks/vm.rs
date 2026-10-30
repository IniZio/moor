//! A Box's VM on a VM host. crabbox and its provider run on the host, so the runner drives them
//! over the host's ssh, from a lease dir under the Box's state dir there. The VM's slug is fixed
//! per Box, so a VM a crashed boot left behind is found by name and deleted.

use std::collections::HashMap;
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
use crate::route::Route;

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
}

impl Vm {
    /// The route into the VM; a VM that never got its words has none, so commands stay on the host.
    pub(super) fn route(&self) -> Route {
        self.words.clone().map_or(Route::Host, Route::Vm)
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
        let old = read(&path)?;
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
            // A VM that never got made cannot be stopped; a warmup with its slug then fails
            // loudly if it still exists, so this error is not the one to report.
            let _ = self.stop(&caller.reach, id, vm).await;
        }
        let vm = Vm {
            provider: provider.to_owned(),
            slug: format!("moor-{id}"),
            words: None,
        };
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
            write(&path, &record).map_err(|e| format!("{refusal}; {e}"))?;
            return Err(refusal);
        }
        record.vm = Some(Vm {
            words: Some(words.clone()),
            ..vm
        });
        write(&path, &record).map_err(|e| format!("could not keep the VM's words: {e}"))?;
        Ok((words, true))
    }

    /// Deletes the Box's VM on the host.
    pub(super) async fn stop(&self, reach: &Reach, id: i64, vm: &Vm) -> Result<(), String> {
        let args = vm.args("stop", "--id", &[]);
        self.crabbox(reach, id, &args).await.map(drop)
    }

    /// Runs crabbox with `args` on the host, from Box `id`'s lease dir; its output when it
    /// exits 0, else an error quoting the tail.
    async fn crabbox(&self, reach: &Reach, id: i64, args: &[String]) -> Result<ExecOutput, String> {
        let state = BoxKey::new(BoxId::new(id)).machine_state_dir();
        let words: Vec<String> = args.iter().map(|a| quote(a)).collect();
        let script = format!(
            "mkdir -p ~/{state}/lease && cd ~/{state}/lease && \
             GIT_CEILING_DIRECTORIES=\"$HOME/{state}\" crabbox {}",
            words.join(" ")
        );
        let out = self.helper(id, reach, &script).await?;
        match out.exit_code {
            Some(0) => Ok(out),
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
