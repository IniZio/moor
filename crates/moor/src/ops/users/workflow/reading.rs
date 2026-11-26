//! The reading a workflow shows: the one it holds, judged against what is true now.

use moor_core::config::parse_repo;
use moor_core::reading::ProxyAllow;
use moor_core::user::UserId;
use moor_core::view::reading::{Current, ReadingView};
use moor_core::workflow::{Workflow, READER_INSTRUCTIONS};

use crate::error::CommandError;
use crate::store::Store;

/// `w`'s reading, as `user`, its owner, may read it: the one it holds, or its stored gates read
/// as effects with no phrases; none for a workflow with a step plan or no reading.
pub(super) fn of(
    store: &Store,
    user: UserId,
    w: &Workflow,
) -> Result<Option<ReadingView>, CommandError> {
    if let Some(id) = store.workflow_reading(user, w.id)? {
        if let Some(reading) = store.reading(user, &id)? {
            let allow = config_allow(w.config.as_deref());
            let rules = store.rules_pins(&w.repos)?;
            let now = Current {
                prompt: &w.prompt,
                repos: &w.repos,
                rules: &rules,
                instructions: READER_INSTRUCTIONS,
                allow: &allow,
            };
            return Ok(Some(ReadingView::of(&reading, &now)));
        }
    }
    Ok(w.gates
        .as_ref()
        .map(|g| ReadingView::from_gates(&w.prompt, &g.doc())))
}

/// The config's `proxy.allow`; empty for no config, or one that does not parse, whose entries
/// the Proxy does not get either.
fn config_allow(config: Option<&str>) -> ProxyAllow {
    config
        .filter(|t| !t.trim().is_empty())
        .and_then(|t| parse_repo("workflow config", t).ok())
        .map(|cfg| cfg.proxy.allow)
        .unwrap_or_default()
}
