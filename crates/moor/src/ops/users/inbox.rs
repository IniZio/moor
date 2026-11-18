//! `inbox`: the caller's inbox with each item's state, and marking items read.

use std::collections::HashMap;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::inbox::{is_resolved, state_of, Facts, InboxItemView, InboxKind, InboxValue};
use moor_core::request::InboxOp;
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::BoxId;
use serde_json::Value;

use super::to_value;
use crate::error::CommandError;
use crate::ops::common::view::rfc3339;
use crate::store::{Store, UserInboxRow};

/// The most items a list holds, newest first.
const LIST_MAX: u32 = 50;

/// Run `op` for `user`; both answer with the inbox as it stands after it.
pub(super) fn run(
    store: &Store,
    user: UserId,
    op: &InboxOp,
) -> Result<serde_json::Value, CommandError> {
    if let InboxOp::Read { ids } = op {
        store.mark_inbox_read(user, ids)?;
    }
    let rows = store.user_inbox(user, LIST_MAX)?;
    let mut boxes: HashMap<BoxId, (bool, Option<i64>)> = HashMap::new();
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        let facts = facts_of(store, row, &mut boxes)?;
        let resolved = is_resolved(row.kind, row.event, &facts);
        items.push(InboxItemView {
            id: row.id,
            box_name: row.box_name.clone(),
            repo: row.repo.clone(),
            kind: row.kind,
            state: state_of(row.read, resolved),
            title: row.title.clone(),
            text: row.text.clone(),
            at: rfc3339(Ts::from_millis(row.created_at)),
        });
    }
    to_value(&InboxValue::of(items))
}

/// What the controller knows now about the wait `row` stands for. Each Box is read once per
/// list, and only for the kinds that need it.
fn facts_of(
    store: &Store,
    row: &UserInboxRow,
    boxes: &mut HashMap<BoxId, (bool, Option<i64>)>,
) -> Result<Facts, CommandError> {
    let mut facts = Facts {
        box_closed: false,
        newest_marker: None,
        run_waits_here: true,
    };
    match row.kind {
        InboxKind::NeedsInput => {
            let (closed, marker) = if let Some(known) = boxes.get(&row.box_id) {
                *known
            } else {
                let closed = store
                    .box_row(row.box_id)?
                    .is_some_and(|b| b.lifecycle == BoxLifecycle::Closed);
                let marker = store.newest_question_marker(row.box_id)?;
                boxes.insert(row.box_id, (closed, marker));
                (closed, marker)
            };
            facts.box_closed = closed;
            facts.newest_marker = marker;
        }
        InboxKind::RunWaiting => {
            let run = serde_json::from_str::<Value>(&row.payload)
                .ok()
                .and_then(|p| p.get("run").and_then(Value::as_i64));
            facts.run_waits_here = match run {
                Some(run) => store.run_waits_since(run, row.created_at)?,
                None => false,
            };
        }
        InboxKind::RunFailed => {}
    }
    Ok(facts)
}
