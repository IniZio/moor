//! One launch: the brief and hook files go into the Box, then the agent's pane opens.

use std::time::Duration;

use moor_core::boxscript::{command_line, quote};
use moor_core::domain::run::SyncMode;
use moor_core::exec_env::ExecEnv;
use moor_core::name::{BoxPath, NameError};
use moor_core::seam::agent_profile::{AgentProfile, BoxSetup, LaunchCtx, ProxyAddr};
use moor_core::seam::driver::{BoxStep, Driver, ExecOutcome, ExecSpec};
use moor_core::seam::engine::{EngineError, LaunchNotice, LaunchReq, SessionRef};
use moor_core::seam::proc::{Deadline, Tool};
use moor_core::seam::surface::{PaneCmd, PaneRef, Surface};
use moor_core::Secret;

use self::environ::{box_user, dirs_in, failed};
use self::mac::{Mac, MacBox};
use self::steps::{Plan, Staged};
use super::nono_release::NonoRelease;
use super::probe::HOOK_FILE;
use super::proxy_env;
use super::sandbox;

/// Where the brief lands, relative to the Box workdir.
const BRIEF_FILE: &str = ".moor/brief.md";
/// Time allowed for removing the credential directory.
const CLEANUP_DEADLINE: Duration = Duration::from_secs(30);
/// Time allowed for checking and installing the sandbox tool on the Box.
const INSTALL_DEADLINE: Duration = Duration::from_secs(300);

/// The Box's operating system and architecture, read when the launch has a Proxy.
type Platform = Option<Result<(String, String), Failure>>;

/// The Box user's home and whether that user is root, the Task's Box on a macOS layout, and the
/// platform.
type Opened = ((BoxPath, bool), Option<MacBox>, Platform);

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
    release: &NonoRelease,
) -> Result<SessionRef, EngineError> {
    let connect = driver
        .connect_argv(&req.handle, &req.worktree)
        .map_err(|e| failed(fail("connect", &e)))?;
    let ((home, root), mac, platform) = open_box(driver, req, &connect).map_err(failed)?;
    let ctx = LaunchCtx {
        model: req.model.clone(),
        workdir: req.workdir.clone(),
        proxy: req.proxy.as_deref().map(|p| ProxyAddr {
            box_port: p.box_port,
            nonce: Secret::new(p.nonce.expose().to_vec()),
            ca_pem: p.ca_pem.clone(),
            box_env: p.box_env.clone(),
            forge: p.forge,
        }),
        home,
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
        mac: mac.as_ref(),
    };
    match launch.deliver(&connect, platform) {
        Ok((pane, warnings)) => Ok(SessionRef {
            id: String::new(),
            handle: req.handle.clone(),
            workdir: req.workdir.clone(),
            worktree: req.worktree.clone(),
            pane: Some(pane),
            sandbox: ctx
                .proxy
                .is_some()
                .then(|| format!("nono {}", sandbox::NONO_VERSION)),
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

/// The credential and tmp directories a launch of `req` creates on its Box.
pub(super) fn dirs(d: &dyn Driver, req: &LaunchReq) -> Result<(BoxPath, BoxPath), EngineError> {
    if let Some(mac) = Mac::find(req) {
        return dirs_in(mac.home(), req).map_err(failed);
    }
    let environ = BoxPath::restore("/proc/self/environ").map_err(|e| failed(fail("home", &e)))?;
    let text = match d.peek_file(&req.handle, &environ, &req.worktree) {
        Ok(Some(text)) => text,
        Ok(None) => return Err(failed(fail("home", &"the Box has no process environment"))),
        Err(e) => return Err(failed(fail("home", &e))),
    };
    dirs_in(&environ::home_in(&text).map_err(failed)?, req).map_err(failed)
}

/// Looks the Box user up, and reads the Box platform when the launch has a Proxy, in one batch.
fn open_box(driver: &dyn Driver, req: &LaunchReq, connect: &[String]) -> Result<Opened, Failure> {
    let mac = Mac::find(req);
    let first = match &mac {
        Some(mac) => mac.prepare_step()?,
        None => {
            BoxStep::Peek(BoxPath::restore("/proc/self/environ").map_err(|e| fail("home", &e))?)
        }
    };
    let mut steps = vec![first];
    if req.proxy.is_some() {
        steps.push(install::platform_step(&install::probe_dir(req)));
    }
    let deadline = Deadline(INSTALL_DEADLINE);
    let mut done = driver
        .batch(&req.handle, connect, &steps, &req.worktree, deadline)
        .into_iter();
    let (user, mac) = match mac {
        Some(m) => {
            let m = m.user(done.next())?;
            ((m.home().clone(), m.root), Some(m))
        }
        None => (box_user(done.next())?, None),
    };
    let platform = req
        .proxy
        .is_some()
        .then(|| install::parse_platform(done.next()));
    Ok((user, mac, platform))
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
    /// Fetches the sandbox tool for a Box that lacks it.
    release: &'a NonoRelease,
    /// The Box user is root.
    root: bool,
    /// The Task's Box, on a macOS layout.
    #[allow(
        dead_code,
        reason = "read by the steps that grant the agent file access"
    )]
    mac: Option<&'a MacBox>,
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
        let mac = Mac::find(req);
        let task = mac.as_ref().map(|m| m.task(&req.nonce));
        proxy_env::exec_env(
            self.cred,
            ctx.proxy.as_ref(),
            strip,
            origin,
            &ctx.workdir,
            task,
        )
        .map_err(|e| fail("script", &e))
    }

    /// The script lines that export the proxy environment; empty when the launch has no Proxy.
    fn proxy_lines(&self) -> Result<Vec<String>, Failure> {
        let Some(addr) = &self.ctx.proxy else {
            return Ok(Vec::new());
        };
        let env = self.profile.env(self.ctx);
        let mac = Mac::find(self.req);
        let task = mac.as_ref().map(|m| m.task(&self.req.nonce));
        let origin = self.req.origin.as_deref();
        proxy_env::script_lines(self.cred, addr, &env, origin, task).map_err(|e| fail("script", &e))
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
        let Staged {
            sandboxed,
            hooks,
            preseed,
        } = staged?;
        let argv = self.argv();
        let exec = self.exec_env()?;
        let lines = self.proxy_lines()?;
        let script = script_stripping(
            (cred, self.tmp),
            ctx,
            &hooks,
            &preseed,
            &argv,
            (&lines, sandboxed.as_ref(), &exec.unset),
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
                    label: format!("agent:{}", req.task.as_str()),
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
    (proxy, sandbox): (&[String], Option<&BoxPath>),
) -> Result<String, NameError> {
    script_stripping(paths, ctx, hooks, preseed, argv, (proxy, sandbox, &[]))
}

/// The line that puts the agent's install directories on the Box's `PATH`, in the launch script
/// and in the check that the agent command exists.
const AGENT_PATH_LINE: &str = "export PATH=\"$HOME/.local/bin:$HOME/.bun/bin:$PATH\"";

/// The script the Box runs under `sh`; the credential directory goes once the brief is
/// installed, and at exit or on a hangup, interrupt or terminate before then. With
/// `proxy` lines the directory stays until the agent exits, since the CA bundle and the CA
/// certificate live in it. With a `sandbox` profile the agent runs under `nono`.
/// The `strip_env` names are unset before anything else runs.
fn script_stripping(
    (cred, tmp): (&BoxPath, &BoxPath),
    ctx: &LaunchCtx,
    hooks: &BoxSetup,
    preseed: &BoxSetup,
    argv: &[String],
    (proxy, sandbox, strip_env): (&[String], Option<&BoxPath>, &[String]),
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
    // The platform's secrets go before anything else runs, sandboxed or not.
    let mut at = 4;
    if !strip_env.is_empty() {
        let names = strip_env.iter().map(|n| quote(n)).collect::<Vec<_>>();
        lines.insert(at, format!("unset {}", names.join(" ")));
        at += 1;
    }
    if proxy.is_empty() {
        lines.insert(at + 1, remove);
    }
    lines.extend(proxy.iter().cloned());
    let launcher = BoxPath::under(ctx.home.as_str(), ".local/bin/nono")?;
    if sandbox.is_some() {
        lines.extend(sandbox::harden_lines());
        lines.extend(sandbox::namespace_lines());
    }
    let agent = sandbox.map_or_else(|| agent.clone(), |at| sandbox::wrap(&launcher, at, &agent));
    let prefix = if sandbox.is_some() {
        format!("${} ", sandbox::NAMESPACE_VAR)
    } else {
        String::new()
    };
    lines.push(format!(
        "cd {} && {prefix}{}",
        quote(workdir),
        command_line(&agent)
    ));
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

mod environ;
mod gh;
mod install;
mod mac;
mod steps;

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "launch/tests/dirs.rs"]
mod dirs_tests;
