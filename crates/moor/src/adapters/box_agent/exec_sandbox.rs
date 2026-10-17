//! The sandbox of the Box execs that are not the agent: setup, teardown and `moor run`.
//!
//! On a macOS Box they run under the same nono profile as the agent, built by the same function,
//! with nono started from the real login home and the exec's environment passed only through the
//! profile's `set_vars`.

use moor_core::name::BoxPath;
use moor_core::seam::driver::{BoxStep, StepDone};
use moor_core::seam::engine::{EngineError, SandboxReq, Sandboxed};
use moor_core::seam::proc::Deadline;

use super::launch::install::{
    check_outcome, parse_platform, platform_step, upload_steps, StepResult,
};
use super::launch::mac::{Mac, MacBox};
use super::launch::profile::{build, ProfileArgs};
use super::launch::{fail, Failure, INSTALL_DEADLINE};
use super::nono_release::sha256_hex;
use super::sandbox;
use super::BoxAgent;

/// How to start `req`'s command under the sandbox, or none when the Box is not a macOS layout.
///
/// # Errors
/// Any step that fails: reaching the Box, reading its platform or user, fetching or checking
/// nono, building or writing the profile.
pub(super) fn sandbox(
    agent: &BoxAgent,
    req: &SandboxReq,
) -> Result<Option<Sandboxed>, EngineError> {
    let Some(mac) = Mac::of(&req.workdir, &req.task, &req.mac_grants.toolchains) else {
        return Ok(None);
    };
    prepare(agent, req, mac)
        .map(Some)
        .map_err(|f| EngineError::Failed {
            step: "sandbox",
            message: f.message,
        })
}

/// Runs the Box steps and returns the sandbox start.
fn prepare(agent: &BoxAgent, req: &SandboxReq, mac: Mac) -> Result<Sandboxed, Failure> {
    let driver = agent.driver.as_ref();
    let connect = (driver.connect_argv(&req.handle, &req.worktree))
        .map_err(|e| fail("sandbox", &format!("could not reach the Box: {e}")))?;
    let batch = |steps: &[BoxStep]| -> Vec<StepResult> {
        let deadline = Deadline(INSTALL_DEADLINE);
        driver.batch(&req.handle, &connect, steps, &req.worktree, deadline)
    };
    let first = [mac.prepare_step()?, platform_step(req.cred.as_str())];
    let mut done = batch(&first).into_iter();
    let mac = mac.user(done.next())?;
    let (os, arch) = parse_platform(done.next())?;
    let bytes = (agent.release.binary(&os, &arch)).map_err(|e| fail("sandbox", &e))?;
    let check = sandbox::check_script(&sha256_hex(&bytes));
    let launcher = nono_path(&mac)?;
    let at = BoxPath::under(req.cred.as_str(), sandbox::PROFILE_FILE)
        .map_err(|e| fail("sandbox", &e))?;
    let put = BoxStep::Put {
        path: at.clone(),
        bytes: profile(req, &mac, &os)?.into_bytes(),
        mode: 0o600,
    };
    let mut results = batch(&[BoxStep::Run(check.clone()), put.clone()]).into_iter();
    let code = check_outcome(results.next())?;
    if code == 0 {
        wrote(results.next())?;
    } else if code == sandbox::NOT_INSTALLED {
        let mut steps = upload_steps(&mac.login_home, &bytes, &check)?;
        steps.push(put);
        let mut results = batch(&steps).into_iter();
        results
            .next()
            .ok_or_else(|| fail("sandbox", &"the sandbox install did not run"))?
            .map_err(|e| fail("sandbox", &format!("the sandbox install failed: {e}")))?;
        match check_outcome(results.next())? {
            0 => wrote(results.next())?,
            code => return Err(fail("sandbox", &sandbox::check_failure(&os, code))),
        }
    } else {
        return Err(fail("sandbox", &sandbox::check_failure(&os, code)));
    }
    Ok(Sandboxed {
        lines: sandbox::harden_lines(),
        prefix: sandbox::prefix(&launcher, &at, req.report),
    })
}

/// The pinned nono in the Box user's real login home.
fn nono_path(mac: &MacBox) -> Result<BoxPath, Failure> {
    BoxPath::under(mac.login_home.as_str(), ".local/bin/nono").map_err(|e| fail("sandbox", &e))
}

/// The profile text: the agent's builder, fed the exec's own environment.
fn profile(req: &SandboxReq, mac: &MacBox, os: &str) -> Result<String, Failure> {
    build(ProfileArgs {
        os,
        workdir: &req.workdir,
        home: mac.home(),
        cred: &req.cred,
        tmp: &mac.layout.tmp,
        env_names: req.set_vars.iter().map(|(n, _)| n.clone()).collect(),
        set_vars: req.set_vars.clone(),
        root: mac.root,
        strip_env: &req.strip_env,
        mac: Some(mac),
        grants: &req.mac_grants,
    })
}

/// Passes the result of the profile `Put` and fails for anything else.
fn wrote(result: Option<StepResult>) -> Result<(), Failure> {
    match result {
        Some(Ok(StepDone::Put)) => Ok(()),
        Some(Err(e)) => Err(fail(
            "sandbox",
            &format!("could not write the profile: {e}"),
        )),
        _ => Err(fail("sandbox", &"the sandbox profile was not written")),
    }
}

#[cfg(test)]
mod tests;
