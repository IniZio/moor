//! The controller's workflow pass, behind the `Moor` facade.

use moor_core::request::{GitHubHookReq, UserCmd, WorkflowCmd};
use moor_core::user::{Caller, Identity, Viewer};
use moor_core::Response;

use super::Moor;
use crate::env::Env;
use crate::ops;
use crate::ops::workflow::{self, CheckJob, StartJob};

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
                crate::cx::owners::checked(|| {
                    let start = |job: StartJob| spawn_start(&env, job);
                    let check = |job: CheckJob| spawn_check(&env, job);
                    workflow::tick(&moor.cx, (&start, &check), draining)
                })
            }
            Err(e) => vec![e.error.to_string()],
        }
    }

    /// This engine as `who`, seeing only their Boxes, when `cmd` starts a Box for them, which a
    /// manual run does through the start the tick uses; else unchanged.
    pub(crate) fn acting_for(self, cmd: &UserCmd, who: Option<&Identity>) -> Self {
        match (cmd, who) {
            (UserCmd::Workflow(WorkflowCmd::RunNow { .. }), Some(who)) => {
                let user = self
                    .cx
                    .store()
                    .ok()
                    .and_then(|s| s.user_of(who).ok().flatten());
                let moor = self.acting_as(who.clone());
                match user {
                    Some(u) => moor.viewing_as(Viewer::User(u)),
                    None => moor,
                }
            }
            _ => self,
        }
    }

    /// Run `github-hook` for `caller`, which must be a front.
    pub(crate) fn call_github_hook(
        &self,
        caller: &Result<Caller, String>,
        req: &GitHubHookReq,
    ) -> Response {
        match ops::github_hook::run(&self.cx, caller, req) {
            Ok(value) => Response::ok("github-hook", Some(value)),
            Err(e) => e.into_response("github-hook", vec![]),
        }
    }

    /// Settles each claim an earlier process left, without starting a Box; returns what to log.
    pub(crate) fn resolve_interrupted_workflow_runs(&self) -> Vec<String> {
        workflow::resolve_interrupted(&self.cx)
    }
}

/// Runs the check `job` on a new thread, so a long check never holds up the pass. Its result is a
/// history line in the store, which the next pass reads.
fn spawn_check(env: &Env, job: CheckJob) {
    let env = env.clone();
    std::thread::spawn(move || {
        crate::cx::owners::checked(|| {
            if let Ok(moor) = Moor::from_env(&env) {
                workflow::run_check(&moor.cx, &job);
            }
        });
    });
}

/// Starts the claimed firing `job` on a new thread, as its owner.
fn spawn_start(env: &Env, job: StartJob) {
    let env = env.clone();
    std::thread::spawn(move || {
        crate::cx::owners::checked(|| start_job(&env, &job));
    });
}

/// Starts `job` as the user it acts for.
fn start_job(env: &Env, job: &StartJob) {
    if let Ok(moor) = Moor::from_env(env) {
        let moor = moor
            .acting_as(job.firing.identity.clone())
            .viewing_as(Viewer::User(job.firing.owner));
        workflow::run_start(&moor.cx, job);
    }
}
