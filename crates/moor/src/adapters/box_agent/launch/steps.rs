//! The Box steps of one launch, run as batches.

use moor_core::name::BoxPath;
use moor_core::seam::agent_profile::BoxSetup;
use moor_core::seam::driver::BoxStep;
use moor_core::seam::proc::Deadline;

use super::super::brief::{render, BriefInput};
use super::super::nono_release::sha256_hex;
use super::super::proxy_env;
use super::super::sandbox::{self, SandboxInput};
use super::install::{check_outcome, parse_platform, upload_and_recheck};
use super::{fail, Failure, Launch, Platform, INSTALL_DEADLINE};

/// Which launch step a batch step belongs to, and so how its failure is reported.
#[derive(Clone, Copy)]
pub(super) enum Tag {
    /// The check that the sandbox tool is the pinned binary.
    Check,
    /// The sandbox profile.
    Profile,
    /// The brief.
    Brief,
    /// A hook or preseed file.
    Hooks,
    /// A proxy file, with the text a failure is reported under.
    Proxy(&'static str),
}

impl Tag {
    /// The failure of this step caused by `e`.
    fn failure(self, e: &impl ToString) -> Failure {
        match self {
            Self::Check | Self::Profile => fail("sandbox", e),
            Self::Brief => fail("brief", e),
            Self::Hooks => fail("hooks", e),
            Self::Proxy(text) => Failure {
                step: "proxy",
                message: format!("{text}: {}", e.to_string()),
            },
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
    /// The pinned binary and the script that checks it, when the check is a step.
    check: Option<(Vec<u8>, String)>,
}

impl Plan {
    /// Adds a step.
    fn push(&mut self, tag: Tag, step: BoxStep) {
        self.tags.push(tag);
        self.steps.push(step);
    }
}

/// What the launch needs once its steps have run.
pub(super) struct Staged {
    /// The sandbox profile on the Box, when the agent runs sandboxed.
    pub(super) sandboxed: Option<BoxPath>,
    /// The agent's hook files and step.
    pub(super) hooks: BoxSetup,
    /// The agent's preseed files and step.
    pub(super) preseed: BoxSetup,
}

impl Launch<'_> {
    /// Adds the sandbox check and profile to `plan` and returns the profile's path.
    fn plan_sandbox(
        &self,
        platform: Platform,
        plan: &mut Plan,
    ) -> Result<Option<BoxPath>, Failure> {
        let Some(addr) = &self.ctx.proxy else {
            return Ok(None);
        };
        let (req, cred) = (self.req, self.cred);
        let (os, arch) = platform.unwrap_or_else(|| parse_platform(None))?;
        let bytes = (self.release.binary(&os, &arch)).map_err(|e| fail("sandbox", &e))?;
        let script = sandbox::check_script(&sha256_hex(&bytes));
        plan.push(Tag::Check, BoxStep::Run(script.clone()));
        plan.check = Some((bytes, script));
        let env_names = (self.profile.env(self.ctx).into_iter().map(|v| v.name)).collect();
        let profile = sandbox::profile(&SandboxInput {
            workdir: &self.ctx.workdir,
            home: &self.ctx.home,
            cred,
            tmp: self.tmp,
            env_names,
            set_vars: addr.box_env.clone(),
            root: self.root,
            strip_env: &req.strip_env,
        });
        let at = BoxPath::under(cred.as_str(), sandbox::PROFILE_FILE)
            .map_err(|e| fail("sandbox", &e))?;
        let put = BoxStep::Put {
            path: at.clone(),
            bytes: profile.into_bytes(),
            mode: 0o600,
        };
        plan.push(Tag::Profile, put);
        Ok(Some(at))
    }

    /// Adds every step after the platform to `plan`, in the order a failure is reported; on an error
    /// `plan` holds the steps before it.
    pub(super) fn plan(&self, platform: Platform, plan: &mut Plan) -> Result<Staged, Failure> {
        let (req, cred, ctx) = (self.req, self.cred, self.ctx);
        let sandboxed = self.plan_sandbox(platform, plan)?;
        let hooks = self
            .profile
            .hook_writer(ctx)
            .map_err(|e| fail("hooks", &e))?;
        let preseed = self.profile.preseed(ctx).map_err(|e| fail("preseed", &e))?;
        let brief = render(&BriefInput {
            task: req.task.as_str(),
            goal: req.goal.as_deref(),
            issue: req.issue.as_deref(),
            branch: &req.branch,
            setup_exit: req.setup_exit,
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
        for file in hooks.files.iter().chain(&preseed.files) {
            let put = BoxStep::Put {
                path: file.path.clone(),
                bytes: file.content.as_bytes().to_vec(),
                mode: file.mode,
            };
            plan.push(Tag::Hooks, put);
        }
        if let Some(addr) = &ctx.proxy {
            let staged = proxy_env::stage_steps(cred, addr).map_err(|e| fail("proxy", &e))?;
            for (step, text) in staged {
                plan.push(Tag::Proxy(text), step);
            }
        }
        Ok(Staged {
            sandboxed,
            hooks,
            preseed,
        })
    }

    /// Runs the steps of `plan` as batches; a missing sandbox tool is uploaded and checked again,
    /// then the rest of the steps run.
    pub(super) fn execute(&self, connect: &[String], plan: &Plan) -> Result<(), Failure> {
        let req = self.req;
        let mut from = 0;
        while let (Some(steps), Some(tags)) = (plan.steps.get(from..), plan.tags.get(from..)) {
            if steps.is_empty() {
                break;
            }
            let deadline = Deadline(INSTALL_DEADLINE);
            let results = (self.driver).batch(&req.handle, connect, steps, &req.worktree, deadline);
            let mut next = plan.steps.len();
            for (i, (result, &tag)) in results.into_iter().zip(tags).enumerate() {
                let Tag::Check = tag else {
                    result.map_err(|e| tag.failure(&e))?;
                    continue;
                };
                match check_outcome(Some(result))? {
                    0 => {}
                    sandbox::NOT_INSTALLED => {
                        if let Some((bytes, script)) = &plan.check {
                            upload_and_recheck(self.driver, req, &self.ctx.home, (bytes, script))?;
                            next = from + i + 1;
                            break;
                        }
                    }
                    code => return Err(fail("sandbox", &sandbox::check_failure(code))),
                }
            }
            from = next;
        }
        Ok(())
    }
}
