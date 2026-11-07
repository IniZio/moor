//! The Box steps of one launch, run as batches.

use moor_core::boxscript::profile::PROFILE_DIR;
use moor_core::boxscript::quote;
use moor_core::name::BoxPath;
use moor_core::seam::agent_adapter::BoxSetup;
use moor_core::seam::driver::{BoxStep, DriverError, ExecOutcome, StepDone};
use moor_core::seam::proc::{Deadline, Tool};

use super::super::agent_env;
use super::super::brief::{render, BriefInput};
use super::super::claude::Claude;
use super::super::codex::Codex;
use super::super::profile_writer::{ProfileCtx, ProfileWriter};
use super::{fail, script_path, Failure, Launch, AGENT_PATH_LINE, BATCH_DEADLINE};

/// Which launch step a batch step belongs to, and so how its failure is reported.
#[derive(Clone, Copy)]
pub(super) enum Tag {
    /// The check that the agent command is on the Box's `PATH`.
    Agent,
    /// The brief.
    Brief,
    /// A hook or preseed file.
    Hooks,
    /// The agent script.
    Script,
}

impl Tag {
    /// The failure of this step caused by `e`.
    fn failure(self, e: &impl ToString) -> Failure {
        match self {
            Self::Agent => fail("agent", e),
            Self::Brief => fail("brief", e),
            Self::Hooks => fail("hooks", e),
            Self::Script => fail("script", e),
        }
    }
}

/// The steps to run after the Box user is known.
#[derive(Default)]
pub(super) struct Plan {
    /// The steps, in order.
    steps: Vec<BoxStep>,
    /// What each step is for.
    tags: Vec<Tag>,
}

impl Plan {
    /// Adds a step.
    fn push(&mut self, tag: Tag, step: BoxStep) {
        self.tags.push(tag);
        self.steps.push(step);
    }
}

/// The script that exits non-zero when the agent command is not on the `PATH` the launch uses.
pub(super) fn agent_check_script() -> String {
    let program = quote(Tool::Agent.program());
    format!("{AGENT_PATH_LINE}; command -v {program} >/dev/null 2>&1")
}

/// The failure of a missing agent command.
fn agent_missing() -> Failure {
    let program = Tool::Agent.program();
    fail(
        "agent",
        &format!(
            "the agent command `{program}` is not on the Box's PATH; install it in the Box image \
             or in the `setup` commands of .moor.yaml"
        ),
    )
}

/// Passes an agent step that exited 0 and fails the launch for any other result.
fn gate(tag: Tag, result: Result<StepDone, DriverError>) -> Result<(), Failure> {
    let code = match result.map_err(|e| tag.failure(&e))? {
        StepDone::Ran {
            outcome: ExecOutcome::Exited { code },
            ..
        } => code,
        _ => -1,
    };
    if code == 0 {
        Ok(())
    } else {
        Err(agent_missing())
    }
}

impl Launch<'_> {
    /// The profile writers' setups, one per agent; none when the Box has no profile.
    fn profile_setups(&self) -> Result<Vec<BoxSetup>, Failure> {
        let Some(manifest) = &self.req.profile else {
            return Ok(Vec::new());
        };
        let home = &self.ctx.home;
        let dir = BoxPath::under(home.as_str(), PROFILE_DIR).map_err(|e| fail("profile", &e))?;
        let ctx = ProfileCtx {
            manifest,
            profile_dir: &dir,
            workdir: &self.ctx.workdir,
            home,
        };
        let writers: [&dyn ProfileWriter; 2] = [&Claude, &Codex];
        writers
            .iter()
            .map(|w| w.write(&ctx).map_err(|e| fail("profile", &e)))
            .collect()
    }

    /// The script lines that export the profile's `env`; none when the Box has no profile.
    pub(super) fn profile_env_lines(&self, strip_env: &[String]) -> Result<Vec<String>, Failure> {
        let Some(manifest) = &self.req.profile else {
            return Ok(Vec::new());
        };
        let home = &self.ctx.home;
        let dir = BoxPath::under(home.as_str(), PROFILE_DIR).map_err(|e| fail("profile", &e))?;
        agent_env::profile_lines(&manifest.claude.env, (home, &dir), strip_env)
            .map_err(|e| fail("script", &e))
    }

    /// Adds every step to `plan`, in the order a failure is reported; on an error `plan` holds the
    /// steps before it.
    pub(super) fn plan(&self, plan: &mut Plan) -> Result<(), Failure> {
        let (req, cred, ctx) = (self.req, self.cred, self.ctx);
        plan.push(Tag::Agent, BoxStep::Run(agent_check_script()));
        let hooks = self
            .profile
            .hook_writer(ctx)
            .map_err(|e| fail("hooks", &e))?;
        let preseed = self.profile.preseed(ctx).map_err(|e| fail("preseed", &e))?;
        let writers = self.profile_setups()?;
        let brief = render(&BriefInput {
            name: req.box_name.as_str(),
            issue: req.issue.as_deref(),
            branch: &req.branch,
            setup_exit: req.setup_exit,
            repo_notes: req.repo_notes.as_deref(),
            shape: req.shape,
            disk_note: req.disk_note.as_deref(),
        });
        let path = BoxPath::under(cred.as_str(), "brief.md").map_err(|e| fail("brief", &e))?;
        let bytes = brief.into_bytes();
        plan.push(
            Tag::Brief,
            BoxStep::Put {
                path,
                bytes,
                mode: 0o600,
            },
        );
        let written = writers.iter().flat_map(|w| &w.files);
        for file in hooks.files.iter().chain(&preseed.files).chain(written) {
            let put = BoxStep::Put {
                path: file.path.clone(),
                bytes: file.content.as_bytes().to_vec(),
                mode: file.mode,
            };
            plan.push(Tag::Hooks, put);
        }
        let script = self.agent_script(&hooks, &preseed, &writers)?;
        let path = script_path(cred).map_err(|e| fail("script", &e))?;
        let put = BoxStep::Put {
            path,
            bytes: script.into_bytes(),
            mode: 0o600,
        };
        plan.push(Tag::Script, put);
        Ok(())
    }

    /// Runs the steps of `plan` as one batch, which stops at its first failed step.
    pub(super) fn execute(&self, connect: &[String], plan: &Plan) -> Result<(), Failure> {
        let deadline = Deadline(BATCH_DEADLINE);
        let results = (self.driver).batch(&self.req.handle, connect, &plan.steps, deadline);
        for (result, &tag) in results.into_iter().zip(&plan.tags) {
            match tag {
                Tag::Agent => gate(tag, result)?,
                _ => result.map(drop).map_err(|e| tag.failure(&e))?,
            }
        }
        Ok(())
    }
}
