//! `changes`: what a Box changed in git against its base. A Box whose machine is up is read now,
//! each repo's patch with it. Any other Box answers what the last turn's receipt saw, unwoken.

use moor_core::changes::{ChangesView, RepoChanges, RepoReceipt};
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::request::ChangesReq;
use moor_core::ErrorKind;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::changes::read_now;
use crate::ops::common::target::{self, Rule};
use crate::ops::common::view::rfc3339;

/// Run `changes`.
pub(crate) fn run(cx: &Cx, req: &ChangesReq) -> Result<ChangesView, CommandError> {
    let store = cx.store()?;
    let target = &req.target;
    let row = target::resolve(store, target, Rule::Latest)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no Box {target}")))?;
    if let Some(repos) = read_now(cx, &row) {
        return Ok(ChangesView {
            live: true,
            why: None,
            at: None,
            repos,
        });
    }
    let why = match row.lifecycle {
        BoxLifecycle::Closed => "the Box is closed",
        _ if row.asleep() => "the Box is asleep",
        _ => "the Box's machine is not up",
    };
    let last = store.newest_events_of(row.id, "turn_changes", 1)?.pop();
    let receipts: Vec<RepoReceipt> = last
        .as_ref()
        .and_then(|e| serde_json::from_value(e.payload.get("repos")?.clone()).ok())
        .unwrap_or_default();
    Ok(ChangesView {
        live: false,
        why: Some(why.to_owned()),
        at: last.map(|e| rfc3339(e.ts)),
        repos: receipts
            .into_iter()
            .map(|r| RepoChanges {
                repo: r.repo,
                total: r.total,
                files: r.files,
                patch: None,
                cut: false,
                problem: None,
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests;
