//! One launch: the brief and hook files go into the Box, then the agent's pane opens.

use std::time::Duration;

use moor_core::boxscript::{command_line, quote};
use moor_core::domain::run::SyncMode;
use moor_core::name::{BoxPath, NameError};
use moor_core::seam::agent_profile::{AgentProfile, BoxSetup, LaunchCtx};
use moor_core::seam::driver::{Driver, ExecOutcome, ExecSpec};
use moor_core::seam::engine::{EngineError, LaunchReq, SessionRef};
use moor_core::seam::proc::{Deadline, Tool};
use moor_core::seam::surface::{PaneCmd, PaneRef, Surface};

use super::brief::{render, BriefInput};
use super::probe::HOOK_FILE;

/// Where the brief lands, relative to the Box workdir.
const BRIEF_FILE: &str = ".moor/brief.md";
/// Time allowed for removing the credential directory.
const CLEANUP_DEADLINE: Duration = Duration::from_secs(30);

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
) -> Result<SessionRef, EngineError> {
    let connect = driver
        .connect_argv(&req.handle, &req.worktree)
        .map_err(|e| failed(fail("connect", &e)))?;
    let home = box_home(driver, req).map_err(failed)?;
    let ctx = LaunchCtx {
        model: req.model.clone(),
        workdir: req.workdir.clone(),
        bypass: false,
        home,
    };
    let cred = BoxPath::under(
        ctx.home.as_str(),
        &format!(".moor-cred-{}-{}", req.task.as_str(), req.nonce),
    )
    .map_err(|e| failed(fail("brief", &e)))?;
    let launch = Launch {
        driver,
        surface,
        profile,
        req,
        ctx: &ctx,
        cred: &cred,
    };
    match launch.deliver(&connect) {
        Ok(pane) => Ok(SessionRef {
            id: String::new(),
            handle: req.handle.clone(),
            workdir: req.workdir.clone(),
            worktree: req.worktree.clone(),
            pane: Some(pane),
        }),
        Err(mut f) => {
            if let Err(c) = remove_cred(driver, req, &cred) {
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
}

impl Launch<'_> {
    /// Puts the files in the Box and opens the tab.
    fn deliver(&self, connect: &[String]) -> Result<PaneRef, Failure> {
        let Self {
            driver,
            surface,
            profile,
            req,
            ctx,
            cred,
        } = self;
        let hooks = profile.hook_writer(ctx).map_err(|e| fail("hooks", &e))?;
        let preseed = profile.preseed(ctx).map_err(|e| fail("preseed", &e))?;
        let brief = render(&BriefInput {
            task: req.task.as_str(),
            goal: req.goal.as_deref(),
            issue: req.issue.as_deref(),
            branch: &req.branch,
            setup_exit: req.setup_exit,
        });
        let brief_at = BoxPath::under(cred.as_str(), "brief.md").map_err(|e| fail("brief", &e))?;
        driver
            .put_file(
                &req.handle,
                &brief_at,
                brief.as_bytes(),
                0o600,
                &req.worktree,
            )
            .map_err(|e| fail("brief", &e))?;
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
        let argv = req
            .resume
            .as_deref()
            .map_or_else(|| profile.argv(ctx), |id| profile.resume_argv(id, ctx));
        let script = script(cred, ctx, &hooks, &preseed, &argv).map_err(|e| fail("script", &e))?;
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

/// The script the Box runs under `sh`; the credential directory goes once the brief is
/// installed, and at exit or on a hangup, interrupt or terminate before then.
fn script(
    cred: &BoxPath,
    ctx: &LaunchCtx,
    hooks: &BoxSetup,
    preseed: &BoxSetup,
    argv: &[String],
) -> Result<String, NameError> {
    let workdir = ctx.workdir.as_str();
    let moor = quote(BoxPath::under(workdir, ".moor")?.as_str());
    let brief = quote(BoxPath::under(workdir, BRIEF_FILE)?.as_str());
    let state = quote(BoxPath::under(workdir, HOOK_FILE)?.as_str());
    let staged = quote(BoxPath::under(cred.as_str(), "brief.md")?.as_str());
    let agent = std::iter::once(Tool::Agent.program().to_owned())
        .chain(argv.iter().cloned())
        .collect::<Vec<_>>();
    Ok([
        format!("trap {} EXIT", quote(&format!("rm -rf {}", quote(cred.as_str())))),
        "trap 'exit 129' HUP".to_owned(),
        "trap 'exit 130' INT".to_owned(),
        "trap 'exit 143' TERM".to_owned(),
        format!("mkdir -p {moor} && install -m 600 {staged} {brief} || exit 1"),
        format!("rm -rf {}", quote(cred.as_str())),
        format!(
            "printf '{{\"state\":\"starting\",\"event\":\"Launch\",\"ts\":%s}}\\n' \"$(date +%s)\" > {state} || exit 1"
        ),
        format!("{} || exit 1", hooks.step),
        preseed.step.clone(),
        "export PATH=\"$HOME/.local/bin:$HOME/.bun/bin:$PATH\"".to_owned(),
        format!("cd {} && {}", quote(workdir), command_line(&agent)),
    ]
    .join("\n"))
}

/// Removes the credential directory on the Box.
fn remove_cred(driver: &dyn Driver, req: &LaunchReq, cred: &BoxPath) -> Result<(), String> {
    let spec = ExecSpec {
        argv: vec!["rm".to_owned(), "-rf".to_owned(), cred.as_str().to_owned()],
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

#[cfg(test)]
mod tests;
