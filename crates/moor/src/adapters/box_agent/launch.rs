//! One launch: the brief and hook files go into the Box, then the agent's pane opens.

use std::time::Duration;

use moor_core::boxscript::{command_line, quote};
use moor_core::domain::run::SyncMode;
use moor_core::exec_env::ExecEnv;
use moor_core::name::{BoxPath, NameError};
use moor_core::seam::agent_profile::{AgentProfile, BoxSetup, LaunchCtx, ProxyAddr};
use moor_core::seam::driver::{BoxStep, Driver, ExecOutcome, ExecSpec, StepDone};
use moor_core::seam::engine::{EngineError, LaunchNotice, LaunchReq, SessionRef};
use moor_core::seam::proc::{Deadline, Tool};
use moor_core::seam::surface::{PaneCmd, PaneRef, Surface};

use self::install::StepResult;
use self::steps::{Plan, Staged};
use super::agent_env;
use super::gh_release::GhRelease;
use super::probe::HOOK_FILE;
use super::proxy_env;

/// Where the brief lands, relative to the Box workdir.
const BRIEF_FILE: &str = ".moor/brief.md";
/// Time allowed for removing the credential directory.
const CLEANUP_DEADLINE: Duration = Duration::from_secs(30);
/// Time allowed for checking and installing the pinned `gh` on the Box.
const INSTALL_DEADLINE: Duration = Duration::from_secs(300);

/// The Box's operating system and architecture, read when the launch has a forge Proxy.
type Platform = Option<Result<(String, String), Failure>>;

/// The pane a launch opened, and its notices.
type Delivered = (PaneRef, Vec<LaunchNotice>);

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
    profile: &dyn AgentProfile,
    req: &LaunchReq,
    release: &GhRelease,
) -> Result<SessionRef, EngineError> {
    let connect = driver
        .connect_argv(&req.handle, &req.worktree)
        .map_err(|e| failed(fail("connect", &e)))?;
    let ((home, root), platform) = open_box(driver, req, &connect).map_err(failed)?;
    let ctx = LaunchCtx {
        model: req.model.clone(),
        workdir: req.workdir.clone(),
        proxy: req.proxy.as_deref().map(|p| ProxyAddr {
            box_port: p.box_port,
            ca_pem: p.ca_pem.clone(),
            box_env: p.box_env.clone(),
            forge: p.forge,
            direct: p.direct.clone(),
        }),
        home,
        prompt: req.prompt.clone(),
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
        release,
        root,
    };
    match launch.deliver(&connect, platform) {
        Ok((pane, warnings)) => Ok(SessionRef {
            id: String::new(),
            handle: req.handle.clone(),
            workdir: req.workdir.clone(),
            worktree: req.worktree.clone(),
            pane: Some(pane),
            warnings,
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
    let text = match d.peek_file(&req.handle, &environ, &req.worktree) {
        Ok(Some(text)) => text,
        Ok(None) => return Err(failed(fail("home", &"the Box has no process environment"))),
        Err(e) => return Err(failed(fail("home", &e))),
    };
    dirs_in(&home_in(&text).map_err(failed)?, req).map_err(failed)
}

/// The Box user's home, and whether that user is root, read from the environment of a process on
/// the Box.
fn box_user(peeked: Option<StepResult>) -> Result<(BoxPath, bool), Failure> {
    let text = match peeked {
        Some(Ok(StepDone::Peeked(Some(text)))) => text,
        Some(Err(e)) => return Err(fail("home", &e)),
        _ => return Err(fail("home", &"the Box has no process environment")),
    };
    Ok((home_in(&text)?, environ_var(&text, "USER") == Some("root")))
}

/// The value of `name` in NUL-separated process environment `text`.
fn environ_var<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!("{name}=");
    text.split('\0').find_map(|kv| kv.strip_prefix(&prefix))
}

/// The Box user's home from NUL-separated process environment `text`.
fn home_in(text: &str) -> Result<BoxPath, Failure> {
    let home = environ_var(text, "HOME")
        .ok_or_else(|| fail("home", &"the Box user has no home directory"))?;
    BoxPath::restore(home).map_err(|e| fail("home", &e))
}

/// Looks the Box user up, and reads the Box platform when the launch has a forge Proxy, in one batch.
fn open_box(
    driver: &dyn Driver,
    req: &LaunchReq,
    connect: &[String],
) -> Result<((BoxPath, bool), Platform), Failure> {
    let environ = BoxPath::restore("/proc/self/environ").map_err(|e| fail("home", &e))?;
    let mut steps = vec![BoxStep::Peek(environ)];
    let forge = req.proxy.as_ref().is_some_and(|p| p.forge);
    if forge {
        steps.push(install::platform_step(&req.dir_names().0));
    }
    let deadline = Deadline(INSTALL_DEADLINE);
    let mut done = driver
        .batch(&req.handle, connect, &steps, &req.worktree, deadline)
        .into_iter();
    let user = box_user(done.next())?;
    let platform = forge.then(|| install::parse_platform(done.next()));
    Ok((user, platform))
}

/// What one launch works with.
struct Launch<'a> {
    /// Reads and writes the Box.
    driver: &'a dyn Driver,
    /// Opens the tab.
    surface: &'a dyn Surface,
    /// Builds the agent's arguments and hook files.
    profile: &'a dyn AgentProfile,
    /// The launch asked for.
    req: &'a LaunchReq,
    /// What the profile needs.
    ctx: &'a LaunchCtx,
    /// The credential directory on the Box.
    cred: &'a BoxPath,
    /// The agent's own temp directory on the Box.
    tmp: &'a BoxPath,
    /// Fetches the pinned `gh` for a Box that lacks it.
    release: &'a GhRelease,
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
        let (strip, origin) = (&req.strip_env, req.origin.as_deref());
        proxy_env::exec_env(ctx.proxy.as_ref(), strip, origin, &ctx.workdir)
            .map_err(|e| fail("script", &e))
    }

    /// The script lines that export the proxy environment; empty when the launch has no Proxy.
    fn proxy_lines(&self) -> Result<Vec<String>, Failure> {
        let Some(addr) = &self.ctx.proxy else {
            return Ok(Vec::new());
        };
        let env = self.profile.env(self.ctx);
        proxy_env::script_lines(addr, &env, self.req.origin.as_deref())
            .map_err(|e| fail("script", &e))
    }

    /// Puts the files in the Box and opens the tab.
    fn deliver(&self, connect: &[String], platform: Platform) -> Result<Delivered, Failure> {
        let Self {
            surface,
            ctx,
            cred,
            req,
            ..
        } = self;
        let mut plan = Plan::default();
        let staged = self.plan(platform, &mut plan);
        let warnings = self.execute(connect, &plan)?;
        let Staged { hooks, preseed } = staged?;
        let argv = self.argv();
        let exec = self.exec_env()?;
        let lines = self.proxy_lines()?;
        let env = agent_env::export_lines(&exec.vars, self.root, &exec.unset);
        let env = env.map_err(|e| fail("script", &e))?;
        let script = script_stripping(
            (cred, self.tmp),
            ctx,
            &hooks,
            &preseed,
            &argv,
            (&lines, &env, &exec.unset),
        );
        let script = script.map_err(|e| fail("script", &e))?;
        let line = format!(
            "{} {}; c=$?; printf '\\r\\033[K'; echo \"moor: agent exited $c\"",
            command_line(connect),
            quote(&command_line(&[script]))
        );
        surface
            .open_pane(
                &req.workspace,
                &PaneCmd {
                    label: format!("agent:{}", req.box_name.as_str()),
                    argv: vec!["sh".to_owned(), "-c".to_owned(), line],
                    cwd: Some(req.worktree.clone()),
                    env: Vec::new(),
                },
            )
            .map(|pane| (pane, warnings))
            .map_err(|e| fail("pane", &e))
    }
}

/// [`script_stripping`] with no names to strip.
#[cfg(test)]
fn script(
    paths: (&BoxPath, &BoxPath),
    ctx: &LaunchCtx,
    hooks: &BoxSetup,
    preseed: &BoxSetup,
    argv: &[String],
    (proxy, env): (&[String], &[String]),
) -> Result<String, NameError> {
    script_stripping(paths, ctx, hooks, preseed, argv, (proxy, env, &[]))
}

/// The line that puts the agent's install directories on the Box's `PATH`, in the launch script
/// and in the check that the agent command exists.
const AGENT_PATH_LINE: &str = "export PATH=\"$HOME/.local/bin:$HOME/.bun/bin:$PATH\"";

/// The script the Box runs under `sh`; the credential directory goes once the brief is
/// installed, and at exit or on a hangup, interrupt or terminate before then. The `strip_env`
/// names and crabbox's own names are unset before anything else runs, and the `env` exports run
/// just before the agent.
fn script_stripping(
    (cred, tmp): (&BoxPath, &BoxPath),
    ctx: &LaunchCtx,
    hooks: &BoxSetup,
    preseed: &BoxSetup,
    argv: &[String],
    (proxy, env, strip_env): (&[String], &[String], &[String]),
) -> Result<String, NameError> {
    let workdir = ctx.workdir.as_str();
    let moor = quote(BoxPath::under(workdir, ".moor")?.as_str());
    let brief = quote(BoxPath::under(workdir, BRIEF_FILE)?.as_str());
    let state = quote(BoxPath::under(workdir, HOOK_FILE)?.as_str());
    let staged = quote(BoxPath::under(cred.as_str(), "brief.md")?.as_str());
    let agent = std::iter::once(Tool::Agent.program().to_owned())
        .chain(argv.iter().cloned())
        .collect::<Vec<_>>();
    let remove = format!("rm -rf {}", quote(cred.as_str()));
    let remove_all = format!("{remove} {}", quote(tmp.as_str()));
    let tmp = quote(tmp.as_str());
    let mut lines = vec![
        format!("trap {} EXIT", quote(&remove_all)),
        "trap 'exit 129' HUP".to_owned(),
        "trap 'exit 130' INT".to_owned(),
        "trap 'exit 143' TERM".to_owned(),
        format!("mkdir -p -m 700 {tmp} && mkdir -p {moor} && install -m 600 {staged} {brief} || exit 1"),
        format!(
            "printf '{{\"state\":\"starting\",\"event\":\"Launch\",\"ts\":%s}}\\n' \"$(date +%s)\" > {state} || exit 1"
        ),
        format!("{} || exit 1", hooks.step),
        preseed.step.clone(),
        AGENT_PATH_LINE.to_owned(),
        format!("export TMPDIR={tmp} CLAUDE_CODE_TMPDIR={tmp}"),
    ];
    // The platform's secrets and crabbox's names go before anything else runs.
    let mut at = 4;
    if !strip_env.is_empty() {
        let names = strip_env.iter().map(|n| quote(n)).collect::<Vec<_>>();
        lines.insert(at, format!("unset {}", names.join(" ")));
        at += 1;
    }
    for line in agent_env::clear_lines() {
        lines.insert(at, line);
        at += 1;
    }
    lines.insert(at + 1, remove);
    lines.extend(proxy.iter().cloned());
    lines.extend(env.iter().cloned());
    lines.push(format!("cd {} && {}", quote(workdir), command_line(&agent)));
    Ok(lines.join("\n"))
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
        sync: SyncMode::None,
        output: None,
        host_cwd: Some(req.worktree.clone()),
        class: None,
    };
    match driver.exec(&req.handle, &spec, Deadline(CLEANUP_DEADLINE)) {
        Ok(ExecOutcome::Exited { code: 0 }) => Ok(()),
        Ok(ExecOutcome::Exited { code }) => Err(format!("rm exited {code}")),
        Ok(ExecOutcome::Interrupted) => Err("rm was interrupted".to_owned()),
        Err(e) => Err(e.to_string()),
    }
}

mod gh;
mod install;
mod steps;

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "launch/tests/dirs.rs"]
mod dirs_tests;
