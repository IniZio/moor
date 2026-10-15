//! One launch: the brief and hook files go into the Box, then the agent's pane opens.

use std::time::Duration;

use moor_core::boxscript::{command_line, quote};
use moor_core::domain::run::SyncMode;
use moor_core::name::{BoxPath, NameError};
use moor_core::seam::agent_profile::{AgentProfile, BoxSetup, LaunchCtx, ProxyAddr};
use moor_core::seam::driver::{Driver, ExecOutcome, ExecSpec};
use moor_core::seam::engine::{EngineError, LaunchReq, SessionRef};
use moor_core::seam::proc::{Deadline, Tool};
use moor_core::seam::surface::{PaneCmd, PaneRef, Surface};
use moor_core::Secret;

use super::brief::{render, BriefInput};
use super::nono_release::NonoRelease;
use super::probe::HOOK_FILE;
use super::proxy_env;
use super::sandbox::{self, SandboxInput};

/// Where the brief lands, relative to the Box workdir.
const BRIEF_FILE: &str = ".moor/brief.md";
/// Time allowed for removing the credential directory.
const CLEANUP_DEADLINE: Duration = Duration::from_secs(30);
/// Time allowed for checking and installing the sandbox tool on the Box.
const INSTALL_DEADLINE: Duration = Duration::from_secs(300);

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
    let home = box_home(driver, req).map_err(failed)?;
    let ctx = LaunchCtx {
        model: req.model.clone(),
        workdir: req.workdir.clone(),
        proxy: req.proxy.as_deref().map(|p| ProxyAddr {
            box_port: p.box_port,
            nonce: Secret::new(p.nonce.expose().to_vec()),
            ca_pem: p.ca_pem.clone(),
            box_env: p.box_env.clone(),
        }),
        home,
    };
    let cred = BoxPath::under(
        ctx.home.as_str(),
        &format!(".moor-cred-{}-{}", req.task.as_str(), req.nonce),
    )
    .map_err(|e| failed(fail("brief", &e)))?;
    let tmp = BoxPath::under(
        ctx.home.as_str(),
        &format!(".moor-tmp-{}-{}", req.task.as_str(), req.nonce),
    )
    .map_err(|e| failed(fail("brief", &e)))?;
    let launch = Launch {
        driver,
        surface,
        profile,
        req,
        ctx: &ctx,
        cred: &cred,
        tmp: &tmp,
        release,
    };
    match launch.deliver(&connect) {
        Ok(pane) => Ok(SessionRef {
            id: String::new(),
            handle: req.handle.clone(),
            workdir: req.workdir.clone(),
            worktree: req.worktree.clone(),
            pane: Some(pane),
            sandbox: ctx
                .proxy
                .is_some()
                .then(|| format!("nono {}", sandbox::NONO_VERSION)),
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

/// The Box user's home, read from the environment of a process on the Box.
fn box_home(driver: &dyn Driver, req: &LaunchReq) -> Result<BoxPath, Failure> {
    let environ = BoxPath::restore("/proc/self/environ").map_err(|e| fail("home", &e))?;
    let text = driver
        .peek_file(&req.handle, &environ, &req.worktree)
        .map_err(|e| fail("home", &e))?
        .ok_or_else(|| fail("home", &"the Box has no process environment"))?;
    let home = text
        .split('\0')
        .find_map(|kv| kv.strip_prefix("HOME="))
        .ok_or_else(|| fail("home", &"the Box user has no home directory"))?;
    BoxPath::restore(home).map_err(|e| fail("home", &e))
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
}

impl Launch<'_> {
    /// The agent's arguments: a fresh start, or a resume of the stored session.
    fn argv(&self) -> Vec<String> {
        let (profile, ctx) = (self.profile, self.ctx);
        let resume = self.req.resume.as_deref();
        resume.map_or_else(|| profile.argv(ctx), |id| profile.resume_argv(id, ctx))
    }

    /// Stages the nonce and the CA certificate and returns the script lines that export the proxy
    /// environment; empty when the launch has no Proxy.
    fn proxy_lines(&self) -> Result<Vec<String>, Failure> {
        let Some(addr) = &self.ctx.proxy else {
            return Ok(Vec::new());
        };
        let (req, cred) = (self.req, self.cred);
        proxy_env::stage(self.driver, &req.handle, cred, addr, &req.worktree)
            .map_err(|e| fail("proxy", &e))?;
        let env = self.profile.env(self.ctx);
        proxy_env::script_lines(cred, addr, &env, req.origin.as_deref())
            .map_err(|e| fail("script", &e))
    }

    /// Makes sure the sandbox tool is on the Box and stages its profile, returning the profile's path; a
    /// launch without a Proxy runs the agent unsandboxed.
    fn sandbox(&self) -> Result<Option<BoxPath>, Failure> {
        let Some(addr) = &self.ctx.proxy else {
            return Ok(None);
        };
        let (req, cred) = (self.req, self.cred);
        install::ensure(self.driver, req, (&self.ctx.home, cred), self.release)?;
        let env_names = (self.profile.env(self.ctx).into_iter().map(|v| v.name)).collect();
        let profile = sandbox::profile(&SandboxInput {
            workdir: &self.ctx.workdir,
            home: &self.ctx.home,
            cred,
            tmp: self.tmp,
            env_names,
            set_vars: addr.box_env.clone(),
            strip_env: &req.strip_env,
        });
        let at = BoxPath::under(cred.as_str(), sandbox::PROFILE_FILE)
            .map_err(|e| fail("sandbox", &e))?;
        self.driver
            .put_file(&req.handle, &at, profile.as_bytes(), 0o600, &req.worktree)
            .map_err(|e| fail("sandbox", &e))?;
        Ok(Some(at))
    }

    /// Puts the brief in the credential directory.
    fn put_brief(&self) -> Result<(), Failure> {
        let (req, cred) = (self.req, self.cred);
        let brief = render(&BriefInput {
            task: req.task.as_str(),
            goal: req.goal.as_deref(),
            issue: req.issue.as_deref(),
            branch: &req.branch,
            setup_exit: req.setup_exit,
        });
        let brief_at = BoxPath::under(cred.as_str(), "brief.md").map_err(|e| fail("brief", &e))?;
        self.driver
            .put_file(
                &req.handle,
                &brief_at,
                brief.as_bytes(),
                0o600,
                &req.worktree,
            )
            .map_err(|e| fail("brief", &e))?;
        Ok(())
    }

    /// Puts the files in the Box and opens the tab.
    fn deliver(&self, connect: &[String]) -> Result<PaneRef, Failure> {
        let Self {
            driver,
            surface,
            profile,
            req,
            ctx,
            cred,
            ..
        } = self;
        let sandboxed = self.sandbox()?;
        let hooks = profile.hook_writer(ctx).map_err(|e| fail("hooks", &e))?;
        let preseed = profile.preseed(ctx).map_err(|e| fail("preseed", &e))?;
        self.put_brief()?;
        for file in hooks.files.iter().chain(&preseed.files) {
            driver
                .put_file(
                    &req.handle,
                    &file.path,
                    file.content.as_bytes(),
                    file.mode,
                    &req.worktree,
                )
                .map_err(|e| fail("hooks", &e))?;
        }
        let argv = self.argv();
        let lines = self.proxy_lines()?;
        let script = script_stripping(
            (cred, self.tmp),
            ctx,
            &hooks,
            &preseed,
            &argv,
            (&lines, sandboxed.as_ref(), &req.strip_env),
        );
        let script = script.map_err(|e| fail("script", &e))?;
        let line = format!(
            "{} {}; echo \"moor: agent exited $?\"",
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
        "export PATH=\"$HOME/.local/bin:$HOME/.bun/bin:$PATH\"".to_owned(),
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
    }
    let agent = sandbox.map_or_else(|| agent.clone(), |at| sandbox::wrap(&launcher, at, &agent));
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

mod install;

#[cfg(test)]
mod tests;
