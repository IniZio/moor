//! Reads the Box platform and puts the pinned sandbox tool on the Box when it is missing.

use moor_core::boxscript::quote;
use moor_core::name::BoxPath;
use moor_core::seam::driver::{BoxStep, DriverError, ExecOutcome, StepDone};
use moor_core::seam::engine::LaunchReq;
use moor_core::task_key::{MacLayout, TaskKey};

use super::{fail, Failure};

/// What a batch reports for one step.
pub(super) type StepResult = Result<StepDone, DriverError>;

/// The step that writes `uname -s -m` under the credential directory `dir` and prints it back.
/// A relative `dir` is a name under the Box user's home, which is not known yet, so the script
/// reads it from `$HOME`; it exits before creating anything when the Box has no readable process
/// environment or no `$HOME`, the cases where the launch fails at its home step and never cleans
/// up. An absolute `dir` is used as it is and never reads `$HOME`.
pub(super) fn platform_step(dir: &str) -> BoxStep {
    if dir.starts_with('/') {
        return BoxStep::Run(format!(
            "[ -r /proc/self/environ ] || exit 1; \
             c={}; mkdir -p \"$c\" && uname -s -m > \"$c/uname\" && cat \"$c/uname\"",
            quote(dir)
        ));
    }
    BoxStep::Run(format!(
        "[ -r /proc/self/environ ] && [ -n \"$HOME\" ] || exit 1; \
         c=\"$HOME\"/{}; mkdir -p \"$c\" && uname -s -m > \"$c/uname\" && cat \"$c/uname\"",
        quote(dir)
    ))
}

/// The exit code of a finished `Run` step.
pub(super) fn check_outcome(result: Option<StepResult>) -> Result<i32, Failure> {
    match result {
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code },
            ..
        })) => Ok(code),
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Interrupted,
            ..
        })) => Err(fail("sandbox", &"the sandbox install was interrupted")),
        Some(Err(e)) => Err(fail("sandbox", &format!("the sandbox install failed: {e}"))),
        _ => Err(fail("sandbox", &"the sandbox install did not run")),
    }
}

/// Where the platform probe stages: the absolute credential directory of a macOS Box layout, else
/// the credential directory name under the Box user's home.
pub(super) fn probe_dir(req: &LaunchReq) -> String {
    MacLayout::find(&req.workdir, &TaskKey::new(&req.task))
        .and_then(|layout| layout.cred_dir(&req.nonce).ok())
        .map_or_else(|| req.dir_names().0, |dir| dir.as_str().to_owned())
}

/// The Box's operating system and architecture, from the result of the [`platform_step`].
pub(super) fn parse_platform(result: Option<StepResult>) -> Result<(String, String), Failure> {
    let stdout = match result {
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 0 },
            stdout,
        })) => stdout,
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code },
            ..
        })) => {
            let message = format!("could not read the Box platform: exit {code}");
            return Err(fail("sandbox", &message));
        }
        other => return Err(check_outcome(other).err().unwrap_or_else(unreadable)),
    };
    let mut words = stdout.split_whitespace();
    match (words.next(), words.next()) {
        (Some(os), Some(arch)) => Ok((os.to_owned(), arch.to_owned())),
        _ => Err(unreadable()),
    }
}

/// The failure for a platform that cannot be read.
fn unreadable() -> Failure {
    fail("sandbox", &"could not read the Box platform")
}

/// The steps that upload the host-fetched `bytes` as the Box user's nono and run `check` again.
pub(super) fn upload_steps(
    home: &BoxPath,
    bytes: &[u8],
    check: &str,
) -> Result<Vec<BoxStep>, Failure> {
    let path = BoxPath::under(home.as_str(), ".local/bin/nono").map_err(|e| fail("sandbox", &e))?;
    let put = BoxStep::Put {
        path,
        bytes: bytes.to_vec(),
        mode: 0o755,
    };
    Ok(vec![put, BoxStep::Run(check.to_owned())])
}
