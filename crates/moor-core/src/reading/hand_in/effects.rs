//! The effects of a read as its phrases add them.

use std::collections::BTreeMap;

use crate::reading::{Effect, EffectId, Line, Origin, Request};

/// The effects of a read as phrases add them: an allow line or a withheld request two phrases
/// name is one effect.
pub(super) struct Effects {
    /// In order: the gates and outcomes, then each line and request as first named.
    pub(super) list: Vec<Effect>,
    /// Each allow line's effect.
    pub(super) allows: BTreeMap<Line, EffectId>,
    /// Each withheld request's effect.
    pub(super) withheld: BTreeMap<Request, EffectId>,
}

impl Effects {
    /// The effect that allows `line`, added when new.
    pub(super) fn allow(&mut self, line: Line) -> EffectId {
        let next = EffectId::nth(self.list.len());
        *self.allows.entry(line.clone()).or_insert_with(|| {
            self.list.push(Effect::Allow {
                id: next,
                rule: line.rule,
                entry: line.entry,
                from: Origin::Table,
            });
            next
        })
    }

    /// The effect that withholds `request`, added when new.
    pub(super) fn withhold(&mut self, request: Request) -> EffectId {
        let next = EffectId::nth(self.list.len());
        *self.withheld.entry(request.clone()).or_insert_with(|| {
            self.list.push(Effect::Withheld {
                id: next,
                request,
                from: Origin::Table,
            });
            next
        })
    }
}
