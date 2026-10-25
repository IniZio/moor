//! Copies a file or directory out of a Box over the Box's shared ssh connection.
//!
//! The Box runs `tar -cf -` on the path and ssh carries the archive back on stdout, so no
//! crabbox process runs per copy. The archive is unpacked as it arrives into a private staging
//! directory beside the destination and renamed into place only once it is complete, so a
//! failed copy leaves no partial destination. Only regular files and directories are taken:
//! an archive holding a link or a special file is refused whole.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use moor_core::boxscript::quote;
use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, Tool};
use rustix::fs::{renameat_with, RenameFlags, CWD};
use tar::EntryType;

use super::peek::ssh_target;
use super::stream::NO_TAR;
use super::{link, touch, Crabbox};

/// Time allowed for one copy.
const COPY_OUT_DEADLINE: Duration = Duration::from_mins(5);
/// Remote exit code that means the source does not exist.
const MISSING: i32 = 44;
/// Name prefix of the staging directories beside a destination.
const STAGE_PREFIX: &str = ".moor-cp-";
/// Age in milliseconds past which a staging directory is left over from a killed copy.
const STALE_AFTER_MS: i64 = 60 * 60 * 1000;

/// Copies `from` in the Box behind `handle` to the host path `to`, or into `to` when it is a
/// directory. Files arrive owner-only (0600) and directories 0700. A copied file replaces a file
/// already at the target; nothing replaces a directory there, and a copied directory replaces
/// nothing.
pub(super) fn copy_out(
    cb: &Crabbox,
    handle: &BoxHandle,
    from: &BoxPath,
    to: &Path,
) -> Result<(), DriverError> {
    let (parent, leaf) = split(from)?;
    let target = target(to, leaf)?;
    let _ = touch::heartbeat(cb, handle);
    let stage = stage_beside(cb, &target)?;
    let copied = fetch(cb, handle, from, parent, leaf, &stage)
        .and_then(|()| publish(&stage.join(leaf), &target));
    let _ = fs::remove_dir_all(&stage);
    copied
}

/// The Box directory holding `from`, and its last component.
fn split(from: &BoxPath) -> Result<(&str, &str), DriverError> {
    let path = from.as_str().trim_end_matches('/');
    let (parent, leaf) = path
        .rsplit_once('/')
        .ok_or_else(|| failed("the source is not absolute"))?;
    if leaf.is_empty() || leaf == "." || leaf == ".." {
        return Err(failed("the source names no file or directory"));
    }
    Ok((if parent.is_empty() { "/" } else { parent }, leaf))
}

/// Where the copy lands: `to`, or `to/<leaf>` when `to` is a directory. A symlink at `to` is
/// refused, so the copy never lands where a link points.
fn target(to: &Path, leaf: &str) -> Result<PathBuf, DriverError> {
    match fs::symlink_metadata(to) {
        Ok(m) if m.file_type().is_symlink() => Err(failed("the destination is a symlink")),
        Ok(m) if m.is_dir() => Ok(to.join(leaf)),
        _ => Ok(to.to_path_buf()),
    }
}

/// A new private directory next to `target`, on the same filesystem so the copy can be renamed.
/// Staging dirs there older than an hour, left by a moor killed mid-copy, go first.
fn stage_beside(cb: &Crabbox, target: &Path) -> Result<PathBuf, DriverError> {
    let parent = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    remove_stale(cb, parent);
    let stage = parent.join(format!(
        "{STAGE_PREFIX}{:08x}{:08x}",
        cb.rng.u32(),
        cb.rng.u32()
    ));
    DirBuilder::new()
        .mode(0o700)
        .create(&stage)
        .map_err(|e| local("cannot stage the copy", &e))?;
    Ok(stage)
}

/// Removes the staging directories in `dir` last changed over an hour ago. No copy runs past
/// its 5 min deadline, so such a directory belongs to none.
fn remove_stale(cb: &Crabbox, dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = cb.clock.now().as_millis();
    for entry in entries.flatten() {
        let staged = entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with(STAGE_PREFIX));
        let changed = entry
            .metadata()
            .ok()
            .filter(fs::Metadata::is_dir)
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_millis()).ok());
        let stale = changed.is_some_and(|at| now.saturating_sub(at) > STALE_AFTER_MS);
        if staged && stale {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Streams `leaf` of the Box directory `parent` into `stage`.
fn fetch(
    cb: &Crabbox,
    handle: &BoxHandle,
    from: &BoxPath,
    parent: &str,
    leaf: &str,
    stage: &Path,
) -> Result<(), DriverError> {
    let words = link::args(cb, handle, ssh_target(cb, handle)?)?;
    let src = quote(from.as_str());
    let remote = format!(
        "test -e {src} || test -L {src} || exit {MISSING}; \
         command -v tar >/dev/null 2>&1 || exit {NO_TAR}; \
         cd -- {} && exec tar -cf - -- {}",
        quote(parent),
        quote(leaf)
    );
    let spec = ProcSpec::new(Tool::Ssh, Deadline(COPY_OUT_DEADLINE))
        .args(words)
        .arg(remote);
    let mut unpacked = Ok(());
    let result = cb.proc.run_reading(&spec, &mut |reader| {
        unpacked = unpack(reader, leaf, stage);
    });
    if let Err(e) = &result {
        link::failed(cb, handle, e);
    }
    match result {
        Ok(()) => unpacked,
        Err(ProcError::NonZero { code: MISSING, .. }) => Err(failed(&format!(
            "{} does not exist in the Box",
            from.as_str()
        ))),
        Err(ProcError::NonZero { code: NO_TAR, .. }) => {
            Err(DriverError::Unsupported("tar on the Box"))
        }
        Err(ProcError::Timeout { after, .. }) => Err(DriverError::Timeout(after)),
        Err(ProcError::Spawn(_)) => Err(DriverError::Unavailable(
            "ssh could not be started".to_owned(),
        )),
        Err(e) => Err(link::lost(cb, handle, &e)
            .or_else(|| unpacked.err())
            .unwrap_or_else(|| failed("the copy broke off"))),
    }
}

/// Unpacks the archive on `reader` into `stage`, refusing any entry outside `leaf` and any entry
/// that is not a regular file or a directory.
fn unpack(reader: &mut dyn Read, leaf: &str, stage: &Path) -> Result<(), DriverError> {
    let broke = |e: &io::Error| local("the copy broke off", e);
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().map_err(|e| broke(&e))? {
        let mut entry = entry.map_err(|e| broke(&e))?;
        let rel = entry.path().map_err(|e| broke(&e))?.into_owned();
        let mut parts = rel.components();
        let inside = parts.next() == Some(Component::Normal(leaf.as_ref()))
            && parts.all(|c| matches!(c, Component::Normal(_)));
        if !inside {
            return Err(failed("the archive names a path outside the source"));
        }
        let dest = stage.join(&rel);
        let made = match entry.header().entry_type() {
            EntryType::Directory => DirBuilder::new().recursive(true).mode(0o700).create(&dest),
            EntryType::Regular | EntryType::Continuous => OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&dest)
                .and_then(|mut out| io::copy(&mut entry, &mut out))
                .map(|_| ()),
            _ => return Err(failed("the source holds a link or a special file")),
        };
        made.map_err(|e| local("cannot write the copy", &e))?;
    }
    Ok(())
}

/// Moves the finished copy at `staged` to `target`. A file replaces a file there; a directory
/// replaces nothing, not even an empty directory.
fn publish(staged: &Path, target: &Path) -> Result<(), DriverError> {
    let Ok(meta) = fs::symlink_metadata(staged) else {
        return Err(failed("the copy arrived empty"));
    };
    let placed = if meta.is_dir() {
        renameat_with(CWD, staged, CWD, target, RenameFlags::NOREPLACE).map_err(io::Error::from)
    } else {
        fs::rename(staged, target)
    };
    placed.map_err(|e| local("cannot place the copy", &e))
}

/// A copy failure saying `why`.
fn failed(why: &str) -> DriverError {
    DriverError::Failed {
        summary: format!("box copy out failed: {why}"),
    }
}

/// A copy failure from a host-side `e`, naming only its kind.
fn local(why: &str, e: &io::Error) -> DriverError {
    failed(&format!("{why} ({})", e.kind()))
}

#[cfg(test)]
mod tests;
