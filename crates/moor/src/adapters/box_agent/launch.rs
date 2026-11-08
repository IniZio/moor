//! One launch: the brief and hook files go into the Box, then the agent's pane opens.

use std::time::Duration;

pub(in crate::adapters::box_agent) use moor_core::boxscript::AGENT_PATH_LINE;
use moor_core::exec_env::ExecEnv;
use moor_core::name::BoxPath;
use moor_core::seam::agent_adapter::{AgentAdapter, BoxSetup, LaunchCtx, ProxyAddr};
use moor_core::seam::driver::{BoxStep, Driver, ExecOutcome, ExecSpec};
use moor_core::seam::engine::{EngineError, LaunchReq, SessionRef};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{PaneCmd, PaneRef, Surface};

use self::environ::{box_user, home_in};
#[cfg(test)]
use self::script::script;
use self::script::{script_path, script_stripping};
use self::steps::Plan;
use super::agent_env;
use super::proxy_env;

/// Where the brief lands, relative to the Box workdir.
const BRIEF_FILE: &str = ".moor/brief.md";
/// The agent script, in the credential directory, which the script removes once it runs.
const SCRIPT_FILE: &str = "agent.sh";
/// Time allowed for removing the credential directory.
const CLEANUP_DEADLINE: Duration = Duration::from_secs(30);
/// Time allowed for one batch of launch steps.
const BATCH_DEADLINE: Duration = Duration::from_secs(300);

/// A failed step and why.
struct Failure {
    /// The step that failed.
    step: &'static str,
    /// What went wrong.
    message: String,
}

/// A failure of `step` caused by `e`.
fn fail(step: &'static str, e: &impl ToString) -> Failure {
    Failure {
        step,
        message: e.to_string(),
    }
}

/// Delivers the brief and hook files, then opens the agent's tab; a failure removes the
/// credential directory and names the step.
pub(super) fn launch(
    driver: &dyn Driver,
    surface: &dyn Surface,
    profile: &dyn AgentAdapter,
    req: &LaunchReq,
) -> Result<SessionRef, EngineError> {
    let connect = driver
        .connect_argv(&req.handle)
        .map_err(|e| failed(fail("connect", &e)))?;
    let (home, root) = open_box(driver, req, &connect).map_err(failed)?;
    let ctx = LaunchCtx {
        mcp: req.mcp.clone(),
        model: req.model.clone(),
        workdir: req.workdir.clone(),
        proxy: req.proxy.as_deref().map(|p| ProxyAddr {
            box_port: p.box_port,
            egress_port: p.egress_port,
            ca_pem: p.ca_pem.clone(),
            box_env: p.box_env.clone(),
            hosts: p.hosts.clone(),
        }),
        home,
        prompt: req.prompt.clone(),
        instructions: BoxPath::under(req.workdir.as_str(), BRIEF_FILE)
            .map_err(|e| failed(fail("brief", &e)))?,
    };
    let (cred, tmp) = dirs_in(&ctx.home, req).map_err(failed)?;
    let launch = Launch {
        driver,
        surface,
        profile,
        req,
        ctx: &ctx,
        cred: &cred,
        tmp: &tmp,
        root,
    };
    match launch.deliver(&connect) {
        Ok(pane) => Ok(SessionRef {
            id: String::new(),
            handle: req.handle.clone(),
            workdir: req.workdir.clone(),
            pane: Some(pane),
        }),
        Err(mut f) => {
            if let Err(c) = remove_cred(driver, req, (&cred, &tmp)) {
                f.message = format!("{}; could not remove {}: {c}", f.message, cred.as_str());
            }
            Err(failed(f))
        }
    }
}

/// The Engine error for `f`.
fn failed(f: Failure) -> EngineError {
    EngineError::Failed {
        step: f.step,
        message: f.message,
    }
}

/// The credential and tmp directories a launch of `req` creates under `home`.
fn dirs_in(home: &BoxPath, req: &LaunchReq) -> Result<(BoxPath, BoxPath), Failure> {
    req.dirs_under(home).map_err(|e| fail("brief", &e))
}

/// The credential and tmp directories a launch of `req` creates on its Box.
pub(super) fn dirs(d: &dyn Driver, req: &LaunchReq) -> Result<(BoxPath, BoxPath), EngineError> {
    let environ = BoxPath::restore("/proc/self/environ").map_err(|e| failed(fail("home", &e)))?;
    let text = match d.peek_file(&req.handle, &environ) {
        Ok(Some(text)) => text,
        Ok(None) => return Err(failed(fail("home", &"the Box has no process environment"))),
        Err(e) => return Err(failed(fail("home", &e))),
    };
    dirs_in(&home_in(&text).map_err(failed)?, req).map_err(failed)
}

/// Looks the Box user up.
fn open_box(
    driver: &dyn Driver,
    req: &LaunchReq,
    connect: &[String],
) -> Result<(BoxPath, bool), Failure> {
    let environ = BoxPath::restore("/proc/self/environ").map_err(|e| fail("home", &e))?;
    let steps = [BoxStep::Peek(environ)];
    let deadline = Deadline(BATCH_DEADLINE);
    let done = driver.batch(&req.handle, connect, &steps, deadline);
    box_user(done.into_iter().next())
}

/// What one launch works with.
struct Launch<'a> {
    /// Reads and writes the Box.
    driver: &'a dyn Driver,
    /// Opens the tab.
    surface: &'a dyn Surface,
    /// Builds the agent's arguments and hook files.
    profile: &'a dyn AgentAdapter,
    /// The launch asked for.
    req: &'a LaunchReq,
    /// What the profile needs.
    ctx: &'a LaunchCtx,
    /// The credential directory on the Box.
    cred: &'a BoxPath,
    /// The agent's own temp directory on the Box.
    tmp: &'a BoxPath,
    /// The Box user is root.
    root: bool,
}

impl Launch<'_> {
    /// The agent's arguments: a fresh start, or a resume of the stored session.
    fn argv(&self) -> Vec<String> {
        let (profile, ctx) = (self.profile, self.ctx);
        let resume = self.req.resume.as_deref();
        resume.map_or_else(|| profile.argv(ctx), |id| profile.resume_argv(id, ctx))
    }

    /// The Box environment the launch builds, from the builder every exec uses.
    pub(super) fn exec_env(&self) -> Result<ExecEnv, Failure> {
        let (req, ctx) = (self.req, self.ctx);
        proxy_env::exec_env(ctx.proxy.as_ref(), &req.strip_env, &ctx.workdir)
            .map_err(|e| fail("script", &e))
    }

    /// The script lines that export the proxy environment; empty when the launch has no Proxy.
    fn proxy_lines(&self) -> Result<Vec<String>, Failure> {
        let Some(addr) = &self.ctx.proxy else {
            return Ok(Vec::new());
        };
        let env = self.profile.env(self.ctx);
        proxy_env::script_lines(addr, &env).map_err(|e| fail("script", &e))
    }

    /// The script the Box runs to start the agent with the staged `hooks`, `preseed` and
    /// profile `writers`.
    fn agent_script(
        &self,
        hooks: &BoxSetup,
        preseed: &BoxSetup,
        writers: &[BoxSetup],
    ) -> Result<String, Failure> {
        let (req, ctx) = (self.req, self.ctx);
        let exec = self.exec_env()?;
        let lines = self.proxy_lines()?;
        let env = agent_env::export_lines(&exec.vars, self.root, &exec.unset);
        let env = env.map_err(|e| fail("script", &e))?;
        let env = [self.profile_env_lines(&exec.unset)?, env].concat();
        script_stripping(
            (self.cred, self.tmp, &req.nonce),
            ctx,
            hooks,
            preseed,
            (&self.argv(), writers),
            (&lines, &env, &exec.unset),
        )
        .map_err(|e| fail("script", &e))
    }

    /// Puts the files in the Box and opens the tab.
    fn deliver(&self, connect: &[String]) -> Result<PaneRef, Failure> {
        let Self {
            surface, ctx, req, ..
        } = self;
        let mut plan = Plan::default();
        let planned = self.plan(&mut plan);
        self.execute(connect, &plan)?;
        planned?;
        let script = script_path(self.cred).map_err(|e| fail("script", &e))?;
        let line = detach::pane_line(connect, ctx.workdir.as_str(), script.as_str())
            .map_err(|e| fail("script", &e))?;
        surface
            .open_pane(
                &req.workspace,
                &PaneCmd {
                    label: format!("agent:{}", req.box_name.as_str()),
                    argv: vec!["sh".to_owned(), "-c".to_owned(), line],
                    env: Vec::new(),
                },
            )
            .map_err(|e| fail("pane", &e))
    }
}

/// Removes the credential and temp directories on the Box.
fn remove_cred(
    driver: &dyn Driver,
    req: &LaunchReq,
    (cred, tmp): (&BoxPath, &BoxPath),
) -> Result<(), String> {
    let spec = ExecSpec {
        argv: ["rm", "-rf", cred.as_str(), tmp.as_str()]
            .map(str::to_owned)
            .to_vec(),
        env: Vec::new(),
        cwd: None,
        output: None,
    };
    match driver.exec(&req.handle, &spec, Deadline(CLEANUP_DEADLINE)) {
        Ok(ExecOutcome::Exited { code: 0 }) => Ok(()),
        Ok(ExecOutcome::Exited { code }) => Err(format!("rm exited {code}")),
        Ok(ExecOutcome::Interrupted) => Err("rm was interrupted".to_owned()),
        Err(e) => Err(e.to_string()),
    }
}

pub(super) mod detach;
mod environ;
pub(super) mod holder;
mod script;
mod steps;

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "launch/tests/dirs.rs"]
mod dirs_tests;
