//! Refs a Box carries: its branch and pull request links.

use moor_core::BoxId;

use super::super::{Store, StoreError};

impl Store {
    /// Record a `kind` ref with `value` on Box `id`; true when it was new.
    ///
    /// # Errors
    /// Returns [`StoreError`] when `kind` is unknown or the database fails.
    pub(crate) fn add_box_ref(
        &self,
        id: BoxId,
        kind: &str,
        value: &str,
    ) -> Result<bool, StoreError> {
        // ON CONFLICT, not OR IGNORE: OR IGNORE would also swallow the kind CHECK.
        let changed = self.writer()?.execute(
            "INSERT INTO box_refs (box, kind, value) VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
            (id.get(), kind, value),
        )?;
        Ok(changed > 0)
    }

    /// Values of Box `id`'s refs of `kind`, in the order they were added.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails.
    #[cfg_attr(not(test), expect(dead_code, reason = "no caller outside tests yet"))]
    pub(crate) fn box_refs(&self, id: BoxId, kind: &str) -> Result<Vec<String>, StoreError> {
        let conn = self.reader()?;
        let mut stmt =
            conn.prepare("SELECT value FROM box_refs WHERE box = ?1 AND kind = ?2 ORDER BY rowid")?;
        let rows = stmt.query_map((id.get(), kind), |row| row.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Boxes carrying a `kind` ref with `value`, ordered by id.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails.
    #[cfg_attr(not(test), expect(dead_code, reason = "no caller outside tests yet"))]
    pub(crate) fn boxes_by_ref(&self, kind: &str, value: &str) -> Result<Vec<BoxId>, StoreError> {
        let conn = self.reader()?;
        let mut stmt =
            conn.prepare("SELECT box FROM box_refs WHERE kind = ?1 AND value = ?2 ORDER BY box")?;
        let rows = stmt.query_map((kind, value), |row| Ok(BoxId::new(row.get(0)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests;
