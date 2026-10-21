//! Archives the agent's transcript in the Box and brings it to the host.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::Duration;

use moor_core::boxscript::quote;
use moor_core::name::BoxPath;
use moor_core::seam::agent_profile::{AgentProfile, NOTHING_TO_ARCHIVE};
use moor_core::seam::driver::{BoxStep, Driver, DriverError, ExecOutcome, StepDone};
use moor_core::seam::engine::{EngineError, SessionRef};
use moor_core::seam::proc::Deadline;

/// Where the archive is built, relative to the Box workdir.
const ARCHIVE: &str = ".moor/transcripts.tgz";

/// Wall-clock allowance for building the archive.
const ARCHIVE_DEADLINE: Duration = Duration::from_secs(15);

/// The largest archive streamed over the batch ssh; a bigger one is copied out instead.
const STREAM_CAP: u64 = 8 * 1024 * 1024;

/// What the stream step prints in place of the archive when it is over the cap.
const TOO_LARGE: &str = "moor: archive too large to stream";

/// The step that prints the archive as base64 text; run in the workdir after the archive is built.
///
/// A path that is not a regular file, or goes through a symlink, exits 4 and prints nothing.
fn stream_script(workdir: &str, cap: u64) -> String {
    let f = quote(ARCHIVE);
    format!(
        "cd {} && f={f} && if test -f \"$f\" && ! test -h \"$f\" && ! test -h .moor; then \
         if [ \"$(wc -c < \"$f\")\" -le {cap} ]; then base64 < \"$f\" | tr -d '\\n'; \
         else echo '{TOO_LARGE}'; fi; \
         else echo 'moor: transcript archive is not a regular file' >&2; exit 4; fi",
        quote(workdir)
    )
}

/// Decodes standard base64 text, ignoring whitespace; `None` when it is malformed.
fn decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits, mut pad) = (0u32, 0u32, 0usize);
    for c in text.bytes().filter(|c| !c.is_ascii_whitespace()) {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => {
                pad += 1;
                continue;
            }
            _ => return None,
        };
        if pad > 0 {
            return None;
        }
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
        }
    }
    (pad <= 2).then_some(out)
}

/// Writes `bytes` to a new file `dest`, mode 0600.
fn write_new(dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dest)?
        .write_all(bytes)
}

/// The archive bytes the stream step printed; none when it was over the cap.
fn streamed(done: Option<Result<StepDone, DriverError>>) -> Result<Option<Vec<u8>>, EngineError> {
    let fail = |why: String| EngineError::Transcript(format!("stream: {why}"));
    let stdout = match done {
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 0 },
            stdout,
        })) => stdout,
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code },
            ..
        })) => return Err(fail(format!("exit {code}"))),
        Some(Err(e)) => return Err(fail(e.to_string())),
        _ => return Err(fail("no result".to_owned())),
    };
    if stdout.trim() == TOO_LARGE {
        return Ok(None);
    }
    decode(&stdout)
        .map(Some)
        .ok_or_else(|| fail("bad base64".to_owned()))
}

/// Builds the archive on the Box and brings it to `dest`.
///
/// One batch builds the archive and prints it as base64; an archive over the cap is copied out.
pub(super) fn save(
    driver: &dyn Driver,
    profile: &dyn AgentProfile,
    s: &SessionRef,
    dest: &Path,
) -> Result<(), EngineError> {
    let fail = |what: &str, why: String| EngineError::Transcript(format!("{what}: {why}"));
    let script = format!(
        "cd {} && {}",
        quote(s.workdir.as_str()),
        profile.transcript_script(ARCHIVE)
    );
    let connect = driver
        .connect_argv(&s.handle)
        .map_err(|e| fail("archive", e.to_string()))?;
    let steps = [
        BoxStep::Run(script),
        BoxStep::Run(stream_script(s.workdir.as_str(), STREAM_CAP)),
    ];
    let mut done = driver
        .batch(&s.handle, &connect, &steps, Deadline(ARCHIVE_DEADLINE))
        .into_iter();
    match done.next() {
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 0 },
            stdout,
        })) => {
            if stdout.trim() == NOTHING_TO_ARCHIVE {
                return Err(EngineError::NothingToArchive);
            }
        }
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code },
            ..
        })) => return Err(fail("archive", format!("exit {code}"))),
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Interrupted,
            ..
        })) => return Err(fail("archive", "interrupted".to_owned())),
        Some(Ok(_)) => return Err(fail("archive", "unexpected result".to_owned())),
        Some(Err(e)) => return Err(fail("archive", e.to_string())),
        None => return Err(fail("archive", "no result".to_owned())),
    }
    if let Some(bytes) = streamed(done.next())? {
        return write_new(dest, &bytes).map_err(|e| fail("write", e.to_string()));
    }
    let from =
        BoxPath::under(s.workdir.as_str(), ARCHIVE).map_err(|e| fail("copy", e.to_string()))?;
    driver
        .copy_out(&s.handle, &from, dest)
        .map_err(|e| fail("copy", e.to_string()))
}

#[cfg(test)]
mod tests;
