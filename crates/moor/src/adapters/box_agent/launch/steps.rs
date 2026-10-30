//! The Box steps of one launch, run as batches.

use moor_core::boxscript::profile::PROFILE_DIR;
use moor_core::boxscript::quote;
use moor_core::name::BoxPath;
use moor_core::seam::agent_adapter::BoxSetup;
use moor_core::seam::driver::{BoxStep, DriverError, ExecOutcome, StepDone};
use moor_core::seam::engine::LaunchNotice;
use moor_core::seam::proc::{Deadline, Tool};

use super::super::brief::{render, BriefInput};
use super::super::claude::Claude;
use super::super::codex::Codex;
use super::super::profile_writer::{ProfileCtx, ProfileWriter};
use super::super::gh_release::sha256_hex;
use super::gh;
use super::install::{parse_platform, StepResult};
use super::{fail, script_path, Failure, Launch, Platform, AGENT_PATH_LINE, INSTALL_DEADLINE};

/// Which launch step a batch step belongs to, and so how its failure is reported.
#[derive(Clone, Copy)]
pub(super) enum Tag {
    /// The check that the agent command is on the Box's `PATH`.
    Agent,
    /// The check that the Box has the pinned `gh`.
    GhCheck,
    /// The upload of the pinned `gh` after the check found it missing.
    GhPut,
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
            Self::GhCheck | Self::GhPut => fail("gh", e),
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
    /// The pinned `gh` binary, when the Box is to have it.
    gh: Option<Vec<u8>>,
    /// Notices raised while planning.
    warnings: Vec<LaunchNotice>,
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
    /// Adds the `gh` check to `plan` when the launch has a forge Proxy; a platform that cannot be
    /// read is a warning, since the agent runs without `gh`.
    fn plan_forge(&self, platform: Platform, plan: &mut Plan) {
        if !self.ctx.proxy.as_ref().is_some_and(|p| p.forge) {
            return;
        }
        match platform.unwrap_or_else(|| parse_platform(None)) {
            Ok((os, arch)) => self.plan_gh((&os, &arch), plan),
            Err(f) => plan.warnings.push(gh::not_delivered(&f.message)),
        }
    }

    /// Adds the `gh` check to `plan`; a `gh` the host cannot fetch is a warning, not a failure.
    fn plan_gh(&self, (os, arch): (&str, &str), plan: &mut Plan) {
        match self.release.gh_binary(os, arch) {
            Ok(bytes) => {
                let script = gh::check_script(&sha256_hex(&bytes));
                plan.push(Tag::GhCheck, BoxStep::Run(script));
                plan.gh = Some(bytes);
            }
            Err(e) => plan.warnings.push(gh::not_delivered(&e)),
        }
    }

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

    /// Adds every step after the platform to `plan`, in the order a failure is reported; on an error
    /// `plan` holds the steps before it.
    pub(super) fn plan(&self, platform: Platform, plan: &mut Plan) -> Result<(), Failure> {
        let (req, cred, ctx) = (self.req, self.cred, self.ctx);
        plan.push(Tag::Agent, BoxStep::Run(agent_check_script()));
        self.plan_forge(platform, plan);
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

    /// Runs the steps of `plan` as batches; a missing `gh` is uploaded at the head of the next
    /// batch, ahead of the rest of the steps. The result holds the notices of the launch.
    pub(super) fn execute(
        &self,
        connect: &[String],
        plan: &Plan,
    ) -> Result<Vec<LaunchNotice>, Failure> {
        let req = self.req;
        let mut warnings = plan.warnings.clone();
        let mut from = 0;
        let mut head: Vec<(Tag, BoxStep)> = Vec::new();
        while let (Some(rest), Some(rest_tags)) = (plan.steps.get(from..), plan.tags.get(from..)) {
            if rest.is_empty() && head.is_empty() {
                break;
            }
            let (mut tags, mut steps): (Vec<Tag>, Vec<BoxStep>) =
                std::mem::take(&mut head).into_iter().unzip();
            let offset = steps.len();
            steps.extend_from_slice(rest);
            tags.extend_from_slice(rest_tags);
            let deadline = Deadline(INSTALL_DEADLINE);
            let results = (self.driver).batch(&req.handle, connect, &steps, deadline);
            let ran = results.len();
            for (result, &tag) in results.into_iter().zip(&tags) {
                match tag {
                    Tag::Agent => gate(tag, result)?,
                    Tag::GhCheck | Tag::GhPut => {
                        self.gh_step(tag, result, plan, (&mut head, &mut warnings));
                    }
                    _ => result.map(drop).map_err(|e| tag.failure(&e))?,
                }
            }
            // A batch stops at its first failed step; what follows it runs in the next batch.
            from = if ran < steps.len() {
                from + ran.saturating_sub(offset)
            } else {
                plan.steps.len()
            };
        }
        Ok(warnings)
    }

    /// Takes the result of a `gh` step: a missing `gh` queues its upload in `head`, and any other
    /// trouble becomes a warning in `warnings`, since the agent can run without `gh`.
    fn gh_step(
        &self,
        tag: Tag,
        result: StepResult,
        plan: &Plan,
        (head, warnings): (&mut Vec<(Tag, BoxStep)>, &mut Vec<LaunchNotice>),
    ) {
        let code = match (tag, result) {
            (_, Err(e)) => Err(e.to_string()),
            (
                Tag::GhCheck,
                Ok(StepDone::Ran {
                    outcome: ExecOutcome::Exited { code },
                    ..
                }),
            ) => Ok(code),
            (Tag::GhCheck, Ok(_)) => Err("the check was interrupted".to_owned()),
            (_, Ok(_)) => Ok(0),
        };
        let put = plan.gh.as_ref().and_then(|bytes| {
            let path = BoxPath::under(self.ctx.home.as_str(), gh::GH_PATH).ok()?;
            let bytes = bytes.clone();
            Some(BoxStep::Put {
                path,
                bytes,
                mode: 0o755,
            })
        });
        match (code, put) {
            (Ok(0), _) => {}
            (Ok(gh::NOT_INSTALLED), Some(put)) if matches!(tag, Tag::GhCheck) => {
                head.push((Tag::GhPut, put));
            }
            (Ok(code), _) => warnings.push(gh::not_delivered(&format!("the check exited {code}"))),
            (Err(message), _) => warnings.push(gh::not_delivered(&message)),
        }
    }
}
