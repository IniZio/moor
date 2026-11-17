//! A machine node's cap and labels, which its row keeps and a person edits.
//!
//! The scheduler reads them through [`Store::nodes`] on every placement, so a write is the next
//! start's rule. Each write is one statement that decides on the row's revision, so two writers
//! cannot both win.

use std::collections::HashMap;

use moor_core::request::CapEdit;
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_schema::tables::UserMachinesV2;
use sea_query::{Expr, ExprTrait as _, Query};

use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::{Store, StoreError};

/// One node's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NodeSettings {
    /// The machine the node is.
    pub(crate) machine_id: i64,
    /// The user who owns the machine; none for a shared node.
    pub(crate) owner: Option<UserId>,
    /// The most live Boxes it holds; none for no cap.
    pub(crate) max_boxes: Option<u32>,
    /// Its labels.
    pub(crate) labels: Vec<String>,
    /// How many times the settings were written; 0 for none.
    pub(crate) revision: u64,
    /// Who wrote them last.
    pub(crate) edited_by: Option<String>,
    /// When they were written last.
    pub(crate) edited_at: Option<Ts>,
}

/// What a write of a node's settings did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Wrote {
    /// The row now holds these.
    Written(NodeSettings),
    /// The revision the writer expected is not the row's, which is `current`; none when the
    /// machine is gone.
    Stale {
        /// The row's revision.
        current: Option<u64>,
    },
}

/// What a write changes.
pub(crate) struct Change<'a> {
    /// What happens to the cap.
    pub(crate) max_boxes: CapEdit,
    /// The new label list; none leaves the labels.
    pub(crate) labels: Option<&'a [String]>,
}

impl Store {
    /// The settings of `owner`'s machine `name`, or of the shared machine of that name when
    /// `owner` is none.
    pub(crate) fn node_settings(
        &self,
        owner: Option<UserId>,
        name: &str,
    ) -> Result<Option<NodeSettings>, StoreError> {
        let mut find = Query::select();
        find.columns([
            UserMachinesV2::MachineId,
            UserMachinesV2::User,
            UserMachinesV2::MaxBoxes,
            UserMachinesV2::Labels,
            UserMachinesV2::SettingsRevision,
            UserMachinesV2::SettingsEditedBy,
            UserMachinesV2::SettingsEditedAt,
        ])
        .from(UserMachinesV2::Table)
        .and_where(Expr::col(UserMachinesV2::Name).eq(name))
        .and_where(owner.map_or_else(
            || Expr::col(UserMachinesV2::User).is_null(),
            |user| Expr::col(UserMachinesV2::User).eq(user.get()),
        ));
        let row = self
            .reader()?
            .row(&mut find, |r| {
                Ok::<_, crate::store::db::Error>((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, Option<u32>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                ))
            })
            .optional()?;
        row.map(|(machine_id, user, max_boxes, labels, revision, by, at)| {
            Ok(NodeSettings {
                machine_id,
                owner: user.map(UserId::new),
                max_boxes,
                labels: serde_json::from_str(&labels)
                    .map_err(|e| StoreError::Corrupt(format!("machine {name} labels: {e}")))?,
                revision: u64::try_from(revision).unwrap_or(0),
                edited_by: by,
                edited_at: at.map(Ts::from_millis),
            })
        })
        .transpose()
    }

    /// Writes `change` to the settings of `node`, which `name` names, as `edited_by`. With `expected`, the
    /// write happens only when the settings are at that revision.
    pub(crate) fn set_node_settings(
        &self,
        node: &NodeSettings,
        name: &str,
        change: &Change<'_>,
        edited_by: &str,
        expected: Option<u64>,
    ) -> Result<Wrote, StoreError> {
        let mut update = Query::update();
        update
            .table(UserMachinesV2::Table)
            .value(
                UserMachinesV2::SettingsRevision,
                Expr::col(UserMachinesV2::SettingsRevision).add(1),
            )
            .value(UserMachinesV2::SettingsEditedBy, edited_by)
            .value(UserMachinesV2::SettingsEditedAt, self.now().as_millis())
            .and_where(Expr::col(UserMachinesV2::MachineId).eq(node.machine_id));
        match change.max_boxes {
            CapEdit::Keep => {}
            CapEdit::Clear => {
                update.value(UserMachinesV2::MaxBoxes, None::<i64>);
            }
            CapEdit::Set(n) => {
                update.value(UserMachinesV2::MaxBoxes, n.get());
            }
        }
        if let Some(labels) = change.labels {
            let labels = serde_json::to_string(labels)
                .map_err(|e| StoreError::Corrupt(format!("node labels: {e}")))?;
            update.value(UserMachinesV2::Labels, labels);
        }
        if let Some(revision) = expected {
            update.and_where(
                Expr::col(UserMachinesV2::SettingsRevision)
                    .eq(i64::try_from(revision).unwrap_or(i64::MAX)),
            );
        }
        if self.writer().run(&mut update)? == 0 {
            let current = self.node_settings(node.owner, name)?.map(|n| n.revision);
            return Ok(Wrote::Stale { current });
        }
        Ok(self
            .node_settings(node.owner, name)?
            .map_or(Wrote::Stale { current: None }, Wrote::Written))
    }

    /// Each machine's settings revision, for the machines whose settings were ever written.
    pub(crate) fn node_revisions(&self) -> Result<HashMap<i64, u64>, StoreError> {
        let rows = self.reader()?.rows(
            Query::select()
                .columns([UserMachinesV2::MachineId, UserMachinesV2::SettingsRevision])
                .from(UserMachinesV2::Table)
                .and_where(Expr::col(UserMachinesV2::SettingsRevision).gt(0)),
            |r| Ok::<_, crate::store::db::Error>((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, revision) = row?;
            out.insert(id, u64::try_from(revision).unwrap_or(0));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
