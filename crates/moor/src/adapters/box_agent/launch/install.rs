//! Puts the pinned sandbox tool on the Box when it is missing.

use moor_core::boxscript::quote;
use moor_core::domain::run::SyncMode;
use moor_core::name::BoxPath;
use moor_core::seam::driver::{Driver, ExecOutcome, ExecSpec};
use moor_core::seam::engine::LaunchReq;
use moor_core::seam::proc::Deadline;

use super::super::nono_release::NonoRelease;
use super::super::sandbox;
use super::{fail, Failure, INSTALL_DEADLINE};

/// Runs `script` under `sh` on the Box and returns its exit code.
fn run(driver: &dyn Driver, req: &LaunchReq, script: String) -> Result<i32, Failure> {
    let spec = ExecSpec {
        argv: vec!["sh".to_owned(), "-c".to_owned(), script],
        env: Vec::new(),
        cwd: None,
        sync: SyncMode::None,
        output: None,
        host_cwd: Some(req.worktree.clone()),
        class: None,
    };
    match driver.exec(&req.handle, &spec, Deadline(INSTALL_DEADLINE)) {
        Ok(ExecOutcome::Exited { code }) => Ok(code),
        Ok(ExecOutcome::Interrupted) => {
            Err(fail("sandbox", &"the sandbox install was interrupted"))
        }
        Err(e) => Err(fail("sandbox", &format!("the sandbox install failed: {e}"))),
    }
}

/// The Box's operating system and architecture, as `uname -s -m` reports them.
fn platform(
    driver: &dyn Driver,
    req: &LaunchReq,
    cred: &BoxPath,
) -> Result<(String, String), Failure> {
    let file = BoxPath::under(cred.as_str(), "uname").map_err(|e| fail("sandbox", &e))?;
    let script = format!(
        "mkdir -p {} && uname -s -m > {}",
        quote(cred.as_str()),
        quote(file.as_str())
    );
    match run(driver, req, script)? {
        0 => {}
        code => {
            let message = format!("could not read the Box platform: exit {code}");
            return Err(fail("sandbox", &message));
        }
    }
    let text = driver
        .peek_file(&req.handle, &file, &req.worktree)
        .map_err(|e| fail("sandbox", &e))?
        .unwrap_or_default();
    let mut words = text.split_whitespace();
    match (words.next(), words.next()) {
        (Some(os), Some(arch)) => Ok((os.to_owned(), arch.to_owned())),
        _ => Err(fail("sandbox", &"could not read the Box platform")),
    }
}

/// Checks the pinned nono runs on the Box; when it is missing, uploads the host-fetched
/// binary and checks again.
pub(super) fn ensure(
    driver: &dyn Driver,
    req: &LaunchReq,
    (home, cred): (&BoxPath, &BoxPath),
    release: &NonoRelease,
) -> Result<(), Failure> {
    match run(driver, req, sandbox::check_script())? {
        0 => return Ok(()),
        sandbox::NOT_INSTALLED => {}
        code => return Err(fail("sandbox", &sandbox::check_failure(code))),
    }
    let (os, arch) = platform(driver, req, cred)?;
    let bytes = release
        .binary(&os, &arch)
        .map_err(|e| fail("sandbox", &e))?;
    let at = BoxPath::under(home.as_str(), ".local/bin/nono").map_err(|e| fail("sandbox", &e))?;
    driver
        .put_file(&req.handle, &at, &bytes, 0o755, &req.worktree)
        .map_err(|e| fail("sandbox", &e))?;
    match run(driver, req, sandbox::check_script())? {
        0 => Ok(()),
        code => Err(fail("sandbox", &sandbox::check_failure(code))),
    }
}
