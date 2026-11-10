//! Response paths: where each checked selection lands in the response, so the linked
//! issues a query reads can be found there.

use super::super::parse::Field;
use super::super::schema;
use super::{Policy, Reason};

impl<'a> Policy<'a> {
    /// Runs `check` one response key deeper, under `field`.
    pub(super) fn under(
        &mut self,
        field: &'a Field,
        check: impl FnOnce(&mut Self) -> Result<(), Reason>,
    ) -> Result<(), Reason> {
        self.path.push(field.key());
        let out = check(self);
        self.path.pop();
        out
    }

    /// Records the response path of a linked issue, and refuses an alias in a linked issue
    /// or its repository, so the response check finds each field under its own name.
    pub(super) fn mark_linked(&mut self, fields: &[&'a Field], ctx: &str) -> Result<(), Reason> {
        if ctx == schema::LINKED {
            self.linked
                .add(self.path.iter().map(|k| (*k).to_owned()).collect());
        }
        if !schema::UNALIASED.contains(&ctx) {
            return Ok(());
        }
        fields
            .iter()
            .find(|f| f.key() != f.name)
            .map_or(Ok(()), |f| {
                Err(Reason::new(format!(
                    "an alias on `{ctx}.{}` is not allowed",
                    f.name
                )))
            })
    }
}
