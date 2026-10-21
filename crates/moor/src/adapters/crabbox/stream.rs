//! Streams many files out of a Box through one ssh process.
//!
//! The Box runs `tar -cf -` and ssh carries the archive back on stdout as raw
//! bytes, so there is no text encoding and every file is binary-safe. The
//! archive is read incrementally and each file is handed to the caller as a
//! reader, so a large file is never held in memory. A Box without tar answers
//! with [`NO_TAR`]; that maps to Unsupported so the caller falls back to
//! copying files one at a time.

use std::collections::HashSet;
use std::io::Read;

use moor_core::boxscript::quote;
use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, Tool};
use tar::EntryType;

use super::peek::ssh_target;
use super::{link, Crabbox};

/// Remote exit code that means the Box has no tar.
pub(super) const NO_TAR: i32 = 45;

/// Streams the files `names` in `dir` of the Box behind `handle`, calling `sink` once per file.
/// `None` is passed for an entry that is not a regular file.
pub(super) fn stream_files(
    cb: &Crabbox,
    handle: &BoxHandle,
    dir: &BoxPath,
    names: &[String],
    deadline: Deadline,
    sink: &mut dyn FnMut(&str, Option<&mut dyn Read>),
) -> Result<(), DriverError> {
    if names.is_empty() {
        return Ok(());
    }
    let words = link::args(cb, handle, ssh_target(cb, handle)?);
    let quoted: Vec<String> = names.iter().map(|n| quote(n)).collect();
    let remote = format!(
        "command -v tar >/dev/null 2>&1 || exit {NO_TAR}; cd -- {} || exit 1; exec tar -cf - -- {}",
        quote(dir.as_str()),
        quoted.join(" ")
    );
    let spec = ProcSpec::new(Tool::Ssh, deadline).args(words).arg(remote);
    let mut handed: HashSet<String> = HashSet::new();
    let mut broken = false;
    let result = cb.proc.run_reading(&spec, &mut |reader| {
        broken = !read_archive(reader, names, &mut handed, sink);
    });
    if let Err(e) = &result {
        link::failed(cb, handle, e);
    }
    match result {
        Ok(()) if !broken => Ok(()),
        Err(ProcError::NonZero { code: NO_TAR, .. }) if handed.is_empty() && !broken => {
            Err(DriverError::Unsupported("tar on the Box"))
        }
        // tar exits non-zero when a file vanished or changed while it was read; the caller
        // checks what arrived and asks again for the rest.
        Err(ProcError::NonZero { .. }) if !broken && !handed.is_empty() => Ok(()),
        Err(ProcError::Timeout { after, .. }) => Err(DriverError::Timeout(after)),
        Err(ProcError::Spawn(e)) if e.kind() == std::io::ErrorKind::Unsupported => {
            Err(DriverError::Unsupported("streaming files"))
        }
        Err(ProcError::Spawn(_)) => Err(DriverError::Unavailable(
            "ssh could not be started".to_owned(),
        )),
        Ok(()) | Err(_) => Err(broke_off()),
    }
}

/// The error for a stream that ended before the archive was complete.
fn broke_off() -> DriverError {
    DriverError::Failed {
        summary: "box file stream broke off".to_owned(),
    }
}

/// Walks the archive on `reader`, handing each wanted entry to `sink`; false when it is unreadable.
fn read_archive(
    reader: &mut dyn Read,
    names: &[String],
    handed: &mut HashSet<String>,
    sink: &mut dyn FnMut(&str, Option<&mut dyn Read>),
) -> bool {
    let mut archive = tar::Archive::new(reader);
    let Ok(entries) = archive.entries() else {
        return false;
    };
    for entry in entries {
        let Ok(mut entry) = entry else {
            return false;
        };
        let Some(name) = entry_name(&entry) else {
            continue;
        };
        if !names.contains(&name) || !handed.insert(name.clone()) {
            continue;
        }
        match entry.header().entry_type() {
            EntryType::Regular | EntryType::Continuous => sink(&name, Some(&mut entry)),
            _ => sink(&name, None),
        }
    }
    true
}

/// The entry's path without a leading `./` or trailing `/`; `None` when it is not UTF-8.
fn entry_name<R: Read>(entry: &tar::Entry<'_, R>) -> Option<String> {
    let path = entry.path().ok()?;
    let text = path.to_str()?;
    let text = text.strip_prefix("./").unwrap_or(text);
    Some(text.trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests;
