//! The crabbox check: the program, its version, and what each node's provider can do.

use std::collections::BTreeSet;
use std::time::Duration;

use moor_core::name::ProviderId;
use moor_core::request::AsReq;
use moor_core::seam::driver::SleepKind;
use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, Tool};
use moor_core::view::node::{ProviderCheck, ProviderReadyValue};
use moor_core::Request;

use super::nodes::Read;
use super::{answer, Check, Inputs};

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

/// One line per provider the controller's nodes name, from the controller's own crabbox.
pub(super) fn controller(i: &Inputs<'_>) -> Vec<Check> {
    if i.target.is_none() {
        return Vec::new();
    }
    let ready =
        match answer::<ProviderReadyValue>(i.ask, Request::ProviderReady(AsReq { identity: None }))
        {
            Ok(v) => v,
            Err(p) => return vec![Check::warn("crabbox", p.detail, p.fix)],
        };
    ready
        .providers
        .into_iter()
        .map(|p| {
            if p.ok {
                return Check::ok(
                    "crabbox",
                    format!("{}: ready on the controller", p.provider),
                );
            }
            let failing: Vec<String> = p.failing.iter().map(describe).collect();
            Check::fail(
                "crabbox",
                format!("{}: {}", p.provider, failing.join(", ")),
                format!(
                    "on the controller host, run `crabbox doctor --provider {}`",
                    p.provider
                ),
            )
        })
        .collect()
}

/// A failed check with its error class and hint.
fn describe(c: &ProviderCheck) -> String {
    let named = c
        .class
        .as_ref()
        .map_or_else(|| c.check.clone(), |k| format!("{} [{k}]", c.check));
    match &c.hint {
        Some(h) => format!("{named} ({h})"),
        None => named,
    }
}
