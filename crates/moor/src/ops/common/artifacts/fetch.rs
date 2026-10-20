//! Brings fresh artifact files out of a Box: one stream, or one copy per file as a fallback.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::Duration;

use moor_core::artifact::ArtifactFile;
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxPath};
use sha2::{Digest, Sha256};

use super::{discard, is_regular_file, skipped, ARTIFACTS_DIR};
use crate::cx::Cx;
use crate::store::{BoxRow, Store};

/// Wall-clock allowance for the one stream that carries every fresh file; large artifacts
/// (videos, build outputs) need minutes on a single connection.
const STREAM_DEADLINE: Duration = Duration::from_secs(600);
/// Size of the buffer a file passes through on its way to disk; a file is never held whole.
const CHUNK: usize = 64 * 1024;

/// What fetching needs: where the Box is, and where files go.
pub(super) struct Fetch<'a> {
    /// The operation context.
    pub cx: &'a Cx,
    /// The store files end up in.
    pub store: &'a Store,
    /// The Box being collected.
    pub row: &'a BoxRow,
    /// The Box's handle.
    pub handle: &'a BoxHandle,
    /// The Box's workdir.
    pub workdir: &'a BoxPath,
    /// The artifact store directory.
    pub dir: &'a Path,
    /// The private staging directory.
    pub staging: &'a Path,
}

impl Fetch<'_> {
    /// Fetches and stores each file; returns how many new versions were stored.
    pub(super) fn all(&self, files: Vec<ArtifactFile>) -> usize {
        if files.is_empty() {
            return 0;
        }
        let Ok(from) = BoxPath::under(self.workdir.as_str(), ARTIFACTS_DIR) else {
            for file in &files {
                self.unusable(&file.name);
            }
            return 0;
        };
        let names: Vec<String> = files.iter().map(|f| f.name.clone()).collect();
        let mut pending = files;
        let mut stored = 0;
        let host = super::host_of(self.row);
        let result = {
            let mut sink = |name: &str, reader: Option<&mut dyn Read>| {
                let Some(i) = pending.iter().position(|f| f.name == name) else {
                    return;
                };
                let file = pending.remove(i);
                if self.streamed(&file, reader) {
                    stored += 1;
                }
            };
            self.cx.driver().stream_files(
                self.handle,
                &from,
                &names,
                host,
                Deadline(STREAM_DEADLINE),
                &mut sink,
            )
        };
        stored + self.settle(result, &pending)
    }

    /// Handles files the stream left over: copies them on `Unsupported`, warns otherwise;
    /// returns how many new versions were stored.
    fn settle(&self, result: Result<(), DriverError>, pending: &[ArtifactFile]) -> usize {
        match result {
            Err(DriverError::Unsupported(_)) => {
                pending.iter().filter(|file| self.copied(file)).count()
            }
            Err(e) => {
                for file in pending {
                    let name = &file.name;
                    skipped(
                        self.cx,
                        &format!(
                            "artifact {name} could not be copied: {e}; it will be tried next time"
                        ),
                    );
                }
                0
            }
            Ok(()) => {
                for file in pending {
                    let name = &file.name;
                    skipped(
                        self.cx,
                        &format!("artifact {name} was not received; it will be tried next time"),
                    );
                }
                0
            }
        }
    }

    /// Writes one streamed file to staging while hashing it, then stores it.
    fn streamed(&self, file: &ArtifactFile, reader: Option<&mut dyn Read>) -> bool {
        let name = &file.name;
        let Some(reader) = reader else {
            self.not_plain(name);
            return false;
        };
        let staged = self.staging.join(name);
        discard(&staged);
        let copied = create_private(&staged).and_then(|mut out| pump(reader, Some(&mut out)));
        let stored = match copied {
            Ok((size, sha)) => self.check_and_store(file, &staged, size, &sha),
            Err(e) => {
                skipped(
                    self.cx,
                    &format!(
                        "artifact {name} could not be copied: {e}; it will be tried next time"
                    ),
                );
                false
            }
        };
        discard(&staged);
        stored
    }

    /// Copies one file out of the Box and stores it; true when a new version was stored.
    fn copied(&self, file: &ArtifactFile) -> bool {
        let name = &file.name;
        let Ok(from) = BoxPath::under(self.workdir.as_str(), &format!("{ARTIFACTS_DIR}/{name}"))
        else {
            self.unusable(name);
            return false;
        };
        let staged = self.staging.join(name);
        discard(&staged);
        let host = super::host_of(self.row);
        if let Err(e) = self.cx.driver().copy_out(self.handle, &from, &staged, host) {
            skipped(
                self.cx,
                &format!("artifact {name} could not be copied: {e}; it will be tried next time"),
            );
            discard(&staged);
            return false;
        }
        let hashed = if is_regular_file(&staged) {
            File::open(&staged)
                .and_then(|mut f| pump(&mut f, None))
                .ok()
        } else {
            None
        };
        let stored = if let Some((size, sha)) = hashed {
            self.check_and_store(file, &staged, size, &sha)
        } else {
            self.not_plain(name);
            false
        };
        discard(&staged);
        stored
    }

    /// Stores a staged file when it still matches the index; true when a new version was stored.
    fn check_and_store(&self, file: &ArtifactFile, staged: &Path, size: u64, sha: &str) -> bool {
        let name = &file.name;
        if size != file.size || sha != file.sha256 {
            skipped(
                self.cx,
                &format!(
                    "artifact {name} changed while it was copied; it will be collected next time"
                ),
            );
            return false;
        }
        match self
            .store
            .add_artifact_staged(self.row.id, name, staged, sha, self.dir)
        {
            Ok(stored) => stored.is_some(),
            Err(e) => {
                skipped(
                    self.cx,
                    &format!("artifact {name} could not be stored: {e}"),
                );
                false
            }
        }
    }

    /// Warns that a file's path cannot be used in the Box.
    fn unusable(&self, name: &str) {
        skipped(
            self.cx,
            &format!("artifact {name} has an unusable path and was skipped"),
        );
    }

    /// Warns that a file was not a regular file when it was copied.
    fn not_plain(&self, name: &str) {
        skipped(
            self.cx,
            &format!("artifact {name} was not a plain file when copied and was skipped"),
        );
    }
}

/// Creates a new file readable by the owner only; fails when something already sits there.
fn create_private(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

/// Reads `reader` to its end through a fixed buffer, writing to `out` when given;
/// returns the byte count and the SHA-256 hex digest.
fn pump(reader: &mut dyn Read, mut out: Option<&mut File>) -> io::Result<(u64, String)> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let Some(chunk) = buf.get(..n) else { continue };
        hasher.update(chunk);
        if let Some(out) = out.as_deref_mut() {
            out.write_all(chunk)?;
        }
        total += n as u64;
    }
    if let Some(out) = out {
        out.flush()?;
    }
    Ok((total, format!("{:x}", hasher.finalize())))
}
