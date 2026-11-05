//! The controller's workflow pass, behind the `Moor` facade.

use moor_core::user::Viewer;

use super::Moor;
use crate::env::Env;
use crate::ops::workflow::{self, StartJob};

impl Moor {
    /// A pass that starts, finishes and expires workflow runs and returns lines to log. Each
    /// start runs on a thread of its own, as its owner, so a slow Box never holds up the pass or
    /// another workflow; the claim it settles is in the store, so no thread needs joining.
    pub(crate) fn workflow_pass(
        env: &Env,
    ) -> impl Fn(&dyn Fn() -> bool) -> Vec<String> + Send + 'static {
        let env = env.clone();
        move |draining| match Self::from_env(&env) {
            Ok(moor) => {
                let env = env.clone();
                workflow::tick(&moor.cx, &|job| spawn_start(&env, job), draining)
            }
            Err(e) => vec![e.error.to_string()],
        }
    }

    /// Settles each claim an earlier process left, without starting a Box; returns what to log.
    pub(crate) fn resolve_interrupted_workflow_runs(&self) -> Vec<String> {
        workflow::resolve_interrupted(&self.cx)
    }
}

/// Starts the claimed firing `job` on a new thread, as its owner.
fn spawn_start(env: &Env, job: StartJob) {
    let env = env.clone();
    std::thread::spawn(move || {
        if let Ok(moor) = Moor::from_env(&env) {
            let moor = moor
                .acting_as(job.identity.clone())
                .viewing_as(Viewer::User(job.owner));
            workflow::run_start(&moor.cx, &job);
        }
    });
}
