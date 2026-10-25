//! Helpers shared by the test suites of the other crates.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

mod egress;

pub use egress::{write_host_guards, FakeEgress, FAKE_TINYPROXY, HOST_GUARD};

/// Writes `contents` to `path` as an executable that can be run at once.
///
/// The bytes are written by a short-lived child process instead of this one. A file this
/// process holds open for writing is copied into any child that another thread forks until
/// that child execs, and exec of the file fails with `ETXTBSY` for as long as one copy is
/// open. The child's descriptor exists only in the child, and it is gone when this returns.
pub fn write_executable(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg("cat > \"$0\" && chmod 755 \"$0\"")
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()?;
    let fed = child.stdin.take().map_or_else(
        || Err(io::Error::other("no stdin pipe to the writer")),
        |mut stdin| stdin.write_all(contents.as_ref()),
    );
    let status = child.wait()?;
    fed?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "writing {} failed: {status}",
            path.display()
        )))
    }
}

/// The longest base for [`short_tempdir`]: a front's socket sits under it at
/// `state/fronts/<name>.sock`, which must fit a Unix socket path (108 bytes on Linux).
const SHORT_BASE_MAX: usize = 24;

/// The base [`short_tempdir`] uses: `TMPDIR` when it is short and not under `/tmp`, else
/// `/var/tmp`.
fn short_base() -> PathBuf {
    let tmp = std::env::temp_dir();
    if tmp.starts_with("/tmp") || tmp.as_os_str().len() > SHORT_BASE_MAX {
        PathBuf::from("/var/tmp")
    } else {
        tmp
    }
}

/// Makes a fresh dir named `<prefix>-<random>` with a short path outside `/tmp`.
///
/// For state that holds a Unix socket and that a front must be able to see: a front's own
/// `/tmp` hides a dir under `/tmp`. The path never depends on the target dir or the checkout,
/// whose lengths vary.
pub fn short_tempdir(prefix: &str) -> io::Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix(&format!("{prefix}-"))
        .tempdir_in(short_base())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "test code: a failed setup step should abort the test"
)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;

    use super::*;

    #[test]
    fn the_file_holds_the_bytes_and_is_executable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tool");
        write_executable(&path, "#!/bin/sh\necho hi\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "#!/bin/sh\necho hi\n"
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o755
        );
        let out = Command::new(&path).output().unwrap();
        assert_eq!(out.stdout, b"hi\n");
    }

    #[test]
    fn a_short_tempdir_is_short_and_outside_tmp() {
        let dir = short_tempdir("probe").unwrap();
        let path = dir.path();
        assert!(!path.starts_with("/tmp"), "{}", path.display());
        assert!(path.is_dir());
        let socket = path.join("state/fronts/a-front-with-a-long-name.sock");
        assert!(socket.as_os_str().len() < 90, "{}", socket.display());
    }

    #[test]
    fn an_unwritable_path_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(write_executable(&dir.path().join("absent/tool"), "x").is_err());
    }

    #[test]
    fn a_fresh_file_runs_at_once_while_other_threads_fork() {
        let dir = tempfile::tempdir().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let forkers: Vec<_> = (0..6)
            .map(|_| {
                let stop = Arc::clone(&stop);
                thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        let _ = Command::new("true").status();
                    }
                })
            })
            .collect();
        let writers: Vec<_> = (0..6)
            .map(|t| {
                let dir = dir.path().to_owned();
                thread::spawn(move || {
                    for i in 0..100 {
                        let path = dir.join(format!("fake-{t}-{i}"));
                        write_executable(&path, "#!/bin/sh\nexit 0\n").unwrap();
                        assert!(Command::new(&path).status().unwrap().success());
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        for f in forkers {
            f.join().unwrap();
        }
    }
}
