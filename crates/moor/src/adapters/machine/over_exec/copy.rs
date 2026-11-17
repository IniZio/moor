//! Copies a file or directory out of a Box place: the place runs `tar -cf -` on it and the archive
//! is unpacked into a private staging directory beside the destination, then renamed into place
//! once complete, so a failed copy leaves no partial destination. Only regular files and
//! directories are taken, so an archive holding a link or a special file is refused whole.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Cursor, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::super::stream::Timer;
use moor_core::boxscript::quote;
use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{Driver, DriverError, ExecOutcome, ExecSpec, Place};
use moor_core::seam::proc::Pipes;
use rustix::fs::{renameat_with, RenameFlags, CWD};
use tar::EntryType;

/// Time allowed for one copy.
const WITHIN: Duration = Duration::from_mins(5);
/// The largest archive a copy takes, which it holds in memory.
const MAX_ARCHIVE: usize = 256 * 1024 * 1024;
/// Remote exit code that means the source does not exist.
const MISSING: i32 = 44;
/// Remote exit code that means the place has no `tar`.
const NO_TAR: i32 = 45;
/// Name prefix of the staging directories beside a destination.
const STAGE_PREFIX: &str = ".moor-cp-";

/// An archive held in memory up to [`MAX_ARCHIVE`]; a write past it fails, which ends the copy.
struct Archive(Vec<u8>);

impl Write for Archive {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.0.len() + buf.len() > MAX_ARCHIVE {
            return Err(io::Error::other("the source is too large to copy"));
        }
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Copies `from` in `place` to the host path `to`, or into `to` when it is a directory. Files
/// arrive owner-only (0600) and directories 0700. A copied file replaces a file already at the
/// target; nothing replaces a directory there, and a copied directory replaces nothing.
pub(in crate::adapters::machine) fn copy_out(
    d: &dyn Driver,
    h: &BoxHandle,
    place: Place,
    from: &BoxPath,
    to: &Path,
) -> Result<(), DriverError> {
    let (parent, leaf) = split(from)?;
    let target = target(to, leaf)?;
    let archive = fetch(d, h, place, from, (parent, leaf))?;
    let stage = stage_beside(&target)?;
    let copied = unpack(&archive, leaf, &stage).and_then(|()| publish(&stage.join(leaf), &target));
    let _ = fs::remove_dir_all(&stage);
    copied
}

/// Has the place archive `leaf` of its directory `parent`.
fn fetch(
    d: &dyn Driver,
    h: &BoxHandle,
    place: Place,
    from: &BoxPath,
    (parent, leaf): (&str, &str),
) -> Result<Vec<u8>, DriverError> {
    let src = quote(from.as_str());
    let script = format!(
        "test -e {src} || test -L {src} || exit {MISSING}; \
         command -v tar >/dev/null 2>&1 || exit {NO_TAR}; \
         cd -- {} && exec tar -cf - -- {}",
        quote(parent),
        quote(leaf)
    );
    let spec = ExecSpec::new(place, vec!["sh".to_owned(), "-c".to_owned(), script]);
    let (mut out, mut err) = (Archive(Vec::new()), io::sink());
    let timer = Timer::after(WITHIN);
    let stop = move || timer.is_up();
    let pipes = Pipes {
        stdin: Box::new(io::empty()),
        stdout: &mut out,
        stderr: &mut err,
        stop: &stop,
    };
    match d.exec_piped(h, &spec, pipes)? {
        ExecOutcome::Exited { code: 0 } => Ok(out.0),
        ExecOutcome::Exited { code: MISSING } => Err(failed(&format!(
            "{} does not exist in the Box",
            from.as_str()
        ))),
        ExecOutcome::Exited { code: NO_TAR } => Err(DriverError::Unsupported("tar on the Box")),
        ExecOutcome::Exited { code } => Err(failed(&format!("tar exited {code}"))),
        ExecOutcome::Interrupted if timer.is_up() => Err(DriverError::Timeout(WITHIN)),
        ExecOutcome::Interrupted => Err(failed("the copy broke off")),
    }
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
fn stage_beside(target: &Path) -> Result<PathBuf, DriverError> {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let parent = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let stage = parent.join(format!(
        "{STAGE_PREFIX}{:x}-{:x}",
        std::process::id(),
        COUNT.fetch_add(1, Ordering::Relaxed)
    ));
    DirBuilder::new()
        .mode(0o700)
        .create(&stage)
        .map_err(|e| local("cannot stage the copy", &e))?;
    Ok(stage)
}

/// Unpacks `archive` into `stage`, refusing any entry outside `leaf` and any entry that is not a
/// regular file or a directory.
fn unpack(archive: &[u8], leaf: &str, stage: &Path) -> Result<(), DriverError> {
    let broke = |e: &io::Error| local("the copy broke off", e);
    let mut archive = tar::Archive::new(Cursor::new(archive));
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
