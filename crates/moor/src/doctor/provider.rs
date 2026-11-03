//! The crabbox check: the program, its version, and what each node's provider can do.

use std::collections::BTreeSet;
use std::time::Duration;

use moor_core::name::ProviderId;
use moor_core::seam::driver::SleepKind;
use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, Tool};

use super::nodes::Read;
use super::{Check, Inputs};

/// Ceiling for `crabbox --version`.
const DEADLINE_SECS: u64 = 10;

/// Where the install steps are.
const INSTALL: &str = "install a -moor build of IniZio/crabbox on PATH (docs/providers.md)";

/// Whether crabbox is installed, new enough, and knows each node's provider.
pub(super) fn check(i: &Inputs<'_>, nodes: &Read) -> Check {
    let spec =
        ProcSpec::new(Tool::Crabbox, Deadline(Duration::from_secs(DEADLINE_SECS))).arg("--version");
    let version = match i.proc.run(&spec) {
        Ok(out) => out.stdout.trim().to_owned(),
        Err(ProcError::Spawn(_) | ProcError::NonZero { code: 127, .. }) => {
            return Check::fail("provider", "crabbox is not on PATH", INSTALL);
        }
        Err(e) => return Check::fail("provider", format!("crabbox --version: {e}"), INSTALL),
    };
    let runtimes: BTreeSet<&str> = nodes.nodes.iter().map(|n| n.runtime.as_str()).collect();
    let mut facts = Vec::new();
    for runtime in &runtimes {
        match i.driver.caps(&ProviderId::new(*runtime)) {
            Ok(caps) => facts.push(format!(
                "{runtime}: sleep {}, cache volumes {}",
                sleep_word(caps.sleep),
                if caps.cache_volumes { "yes" } else { "no" }
            )),
            Err(e) => {
                return Check::fail("provider", format!("{version}; {runtime}: {e}"), INSTALL);
            }
        }
    }
    if facts.is_empty() {
        return Check::ok(
            "provider",
            format!("{version}; no node names a provider yet"),
        );
    }
    Check::ok("provider", format!("{version}; {}", facts.join("; ")))
}

/// How a provider sleeps a Box, in words.
const fn sleep_word(kind: SleepKind) -> &'static str {
    match kind {
        SleepKind::Suspend => "suspend",
        SleepKind::Native => "native",
        SleepKind::None => "none",
    }
}
