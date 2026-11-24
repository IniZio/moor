//! The controller's place keeper pass, behind the `Moor` facade.

use std::cell::RefCell;

use moor_core::user::Viewer;

use super::Moor;
use crate::env::Env;
use crate::ops;

impl Moor {
    /// A pass that keeps one unclaimed agent place for each user on the `agent_places` list and
    /// returns lines to log. Each start runs as its user, one after another, on the pass's thread:
    /// the Box it inserts is what the next pass reads, so no pass starts a second.
    pub(crate) fn place_pass(
        env: &Env,
    ) -> impl Fn(&dyn Fn() -> bool) -> Vec<String> + Send + 'static {
        let env = env.clone();
        let keeper = ops::PlaceKeeper::default();
        move |draining| match Self::from_env(&env) {
            Ok(moor) => crate::cx::owners::checked(|| {
                let warnings = RefCell::new(Vec::new());
                let start = |user, identity: &_, req: &_| {
                    let acting = Self::from_env(&env)?
                        .acting_as(Clone::clone(identity))
                        .viewing_as(Viewer::User(user));
                    let started = ops::start_box(&acting.cx, req);
                    warnings
                        .borrow_mut()
                        .extend(acting.cx.take_warnings().into_iter().map(|w| w.message));
                    started
                };
                let mut out = match ops::keep_places(&moor.cx, &keeper, &start, draining) {
                    Ok(lines) => lines,
                    Err(e) => vec![e.error.to_string()],
                };
                out.extend(warnings.into_inner());
                out.extend(moor.cx.take_warnings().into_iter().map(|w| w.message));
                out
            }),
            Err(e) => vec![e.error.to_string()],
        }
    }
}
