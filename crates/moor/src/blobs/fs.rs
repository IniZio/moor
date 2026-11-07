//! Blobs as owner-only files under the state directory, each at its key's path.

use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use moor_core::controller::BYTES_MAX;

use super::{BlobError, BlobKey, Blobs};

/// Blobs under one root directory.
pub(crate) struct FsBlobs {
    /// The directory keys are relative to.
    root: PathBuf,
}

impl FsBlobs {
    /// The blobs under `root`.
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    /// The path of `key`, once every part of it below the root is checked: a symlink anywhere
    /// is refused, as is anything but a directory on the way or a regular file at the end.
    fn checked(&self, key: &BlobKey) -> Result<PathBuf, BlobError> {
        let mut cur = self.root.clone();
        let mut parts = key.as_str().split('/').peekable();
        while let Some(part) = parts.next() {
            cur.push(part);
            let meta = fs::symlink_metadata(&cur).map_err(|e| failed(&cur, &e))?;
            let fits = if parts.peek().is_some() {
                meta.is_dir()
            } else {
                meta.is_file()
            };
            if !fits {
                return Err(BlobError::NotPlain(format!(
                    "refusing to use {}: not a plain stored file",
                    cur.display()
                )));
            }
        }
        Ok(cur)
    }

    /// The path to write `key` at, its directories made owner-only.
    fn writable(&self, key: &BlobKey) -> Result<PathBuf, BlobError> {
        let path = match self.checked(key) {
            Ok(path) => path,
            Err(BlobError::Missing) => self.root.join(key.as_str()),
            Err(e) => return Err(e),
        };
        if let Some(dir) = path.parent() {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|e| failed(dir, &e))?;
        }
        Ok(path)
    }
}

impl Blobs for FsBlobs {
    fn put(&self, key: &BlobKey, bytes: &[u8]) -> Result<(), BlobError> {
        let path = self.writable(key)?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let temp = path.with_file_name(format!(".{}.tmp", name.unwrap_or_default()));
        if fs::symlink_metadata(&temp).is_ok_and(|m| m.is_symlink()) {
            return Err(BlobError::NotPlain(format!(
                "refusing to write through symlink {}",
                temp.display()
            )));
        }
        let wrote = write_private(&temp, bytes).and_then(|()| fs::rename(&temp, &path));
        wrote.map_err(|e| failed(&path, &e))
    }

    fn put_file(&self, key: &BlobKey, from: &Path) -> Result<(), BlobError> {
        let path = self.writable(key)?;
        move_private(from, &path).map_err(|e| failed(&path, &e))
    }

    fn size(&self, key: &BlobKey) -> Result<u64, BlobError> {
        let path = self.checked(key)?;
        let meta = fs::metadata(&path).map_err(|e| failed(&path, &e))?;
        Ok(meta.len())
    }

    fn read(
        &self,
        key: &BlobKey,
        offset: u64,
        len: u64,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> Result<(), BlobError> {
        let path = self.checked(key)?;
        let mut file = File::open(&path).map_err(|e| failed(&path, &e))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| failed(&path, &e))?;
        let mut file = file.take(len);
        let mut buf = vec![0; BYTES_MAX];
        loop {
            let n = match file.read(&mut buf) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                n => n.map_err(|e| failed(&path, &e))?,
            };
            let Some(piece) = buf.get(..n).filter(|p| !p.is_empty()) else {
                return Ok(());
            };
            sink(piece).map_err(BlobError::Sink)?;
        }
    }

    fn locate(&self, key: &BlobKey) -> String {
        self.root.join(key.as_str()).to_string_lossy().into_owned()
    }

    fn name(&self) -> &'static str {
        "local"
    }
}

/// The blob error for a file operation on `path`.
fn failed(path: &Path, e: &io::Error) -> BlobError {
    if e.kind() == io::ErrorKind::NotFound {
        return BlobError::Missing;
    }
    BlobError::Store(format!("{}: {}", path.display(), e.kind()))
}

/// Writes `bytes` to `file`, readable by the owner only, and flushes them to disk.
fn write_private(file: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut out = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(file)?;
    out.write_all(bytes)?;
    out.sync_all()?;
    fs::set_permissions(file, Permissions::from_mode(0o600))
}

/// Moves the regular file `from` to `to`, readable by the owner only.
///
/// Falls back to copy and remove when the two paths are on different filesystems.
fn move_private(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            fs::copy(from, to)?;
            fs::remove_file(from)?;
        }
        Err(e) => return Err(e),
    }
    fs::set_permissions(to, Permissions::from_mode(0o600))
}
