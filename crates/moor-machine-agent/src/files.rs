//! Files into and out of a Box's dirs: a sync's `rsync --server`, and a pull of one file.
//!
//! Every path is canonicalized and must lie under the Box's dir or state dir, so neither `..`,
//! an absolute path nor a symlink reaches anything else of the account's.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use moor_client::link::ops::{self, Reply, MAX_PULL};
use moor_client::link::Channel;
use moor_core::box_key::BoxKey;
use moor_core::BoxId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::serve::Machine;

/// Letters a receiving server's option cluster may hold: what `-a`, `-v`, `-z` and the like
/// turn into. None takes an argument or names a path.
const SHORT: &str = "logDtprvzcHSxuWnIKLk";
/// Long options a receiving server may get.
const LONG: [&str; 1] = ["--delete"];

/// The Box's dir and state dir under `home`.
fn dirs(home: &Path, box_id: i64) -> (String, PathBuf, PathBuf) {
    let key = BoxKey::new(BoxId::new(box_id));
    let dir = key.machine_dir();
    (
        dir.clone(),
        home.join(&dir),
        home.join(key.machine_state_dir()),
    )
}

/// The receiving server's options, when `argv` is `rsync --server` aimed at `dest`.
///
/// # Errors
/// It is not that grammar: another program, a sender, an option off the allow-list, or
/// another destination.
pub(crate) fn server_options(argv: &[String], dest: &str) -> Result<Vec<String>, String> {
    let refuse = |why: &str| Err(format!("not a sync into the Box's dir: {why}"));
    let [rsync, server, rest @ ..] = argv else {
        return refuse("too short");
    };
    if rsync != "rsync" || server != "--server" {
        return refuse("not `rsync --server`");
    }
    let [options @ .., dot, to] = rest else {
        return refuse("no destination");
    };
    if dot != "." {
        return refuse("not a receiving server");
    }
    if to.trim_end_matches('/') != dest {
        return refuse(&format!("destination {to:?} is not {dest}"));
    }
    for option in options {
        if !(LONG.contains(&option.as_str()) || short(option)) {
            return refuse(&format!("option {option:?} is not allowed"));
        }
    }
    Ok(options.to_vec())
}

/// Whether `option` is one cluster of allowed letters, perhaps ending in the protocol's `e.…`.
fn short(option: &str) -> bool {
    let Some(cluster) = option.strip_prefix('-').filter(|c| !c.starts_with('-')) else {
        return false;
    };
    let (letters, protocol) = cluster.split_once('e').unwrap_or((cluster, ""));
    !letters.is_empty()
        && letters.chars().all(|c| SHORT.contains(c))
        && protocol
            .chars()
            .all(|c| c == '.' || c.is_ascii_alphanumeric())
}

/// `dir` made and canonicalized, when it is a real directory directly under `root`.
fn confined(root: &Path, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(root).map_err(|e| format!("cannot make {}: {e}", root.display()))?;
    if std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!("{} is a symlink", dir.display()));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let (root, real) = (std::fs::canonicalize(root), std::fs::canonicalize(dir));
    match (root, real) {
        (Ok(root), Ok(real)) if real.parent() == Some(root.as_path()) => Ok(real),
        _ => Err(format!("{} is outside the Box's dirs", dir.display())),
    }
}

/// Runs the receiving `rsync --server` into Box `box_id`'s dir on `ch`, if `argv` asks for
/// nothing else.
pub(crate) async fn sync(machine: &Machine, mut ch: Channel, box_id: i64, argv: &[String]) {
    let (rel, dir, _) = dirs(&machine.home, box_id);
    let root = dir.parent().map(Path::to_path_buf).unwrap_or_default();
    let ready = server_options(argv, &rel).and_then(|o| Ok((o, confined(&root, &dir)?)));
    let (options, canon) = match ready {
        Ok(ready) => ready,
        Err(reason) => return refuse(ch, reason).await,
    };
    let spawned = Command::new("rsync")
        .arg("--server")
        .args(options)
        .arg(".")
        .arg(format!("{}/", canon.display()))
        .current_dir(&canon)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => return refuse(ch, format!("cannot start rsync: {e}")).await,
    };
    let (Some(mut stdin), Some(mut stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return;
    };
    if ops::write_line(&mut ch, &Reply::Started).await.is_err() {
        return;
    }
    let (mut rd, mut wr) = tokio::io::split(ch);
    let to = tokio::spawn(async move {
        let _ = tokio::io::copy(&mut rd, &mut stdin).await;
    });
    let _ = tokio::io::copy(&mut stdout, &mut wr).await;
    let _ = wr.shutdown().await;
    to.abort();
    let _ = child.kill().await;
}

/// The file `path` names in Box `box_id`'s dirs, canonicalized, with its length.
fn pulled(home: &Path, box_id: i64, path: &str) -> Result<(PathBuf, u64), String> {
    let (_, dir, state) = dirs(home, box_id);
    if path.is_empty() || path.contains('\0') {
        return Err(format!("{path:?} does not name a file"));
    }
    let real =
        std::fs::canonicalize(dir.join(path)).map_err(|e| format!("{path} cannot be read: {e}"))?;
    let inside = [dir, state]
        .iter()
        .filter_map(|d| std::fs::canonicalize(d).ok())
        .any(|d| real.starts_with(d));
    if !inside {
        return Err(format!("{path} is outside the Box's dirs"));
    }
    let meta = std::fs::metadata(&real).map_err(|e| format!("{path} cannot be read: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{path} is not a regular file"));
    }
    if meta.len() > MAX_PULL {
        return Err(format!("{path} is larger than {MAX_PULL} bytes"));
    }
    Ok((real, meta.len()))
}

/// Sends the file `path` names in Box `box_id`'s dirs on `ch`.
pub(crate) async fn pull(machine: &Machine, mut ch: Channel, box_id: i64, path: &str) {
    let opened = match pulled(&machine.home, box_id, path) {
        Ok((real, bytes)) => tokio::fs::File::open(&real)
            .await
            .map(|f| (f, bytes))
            .map_err(|e| format!("{path} cannot be read: {e}")),
        Err(e) => Err(e),
    };
    let (file, bytes) = match opened {
        Ok(opened) => opened,
        Err(reason) => return refuse(ch, reason).await,
    };
    if ops::write_line(&mut ch, &Reply::Sending { bytes })
        .await
        .is_ok()
    {
        let _ = tokio::io::copy(&mut file.take(bytes), &mut ch).await;
    }
    let _ = ch.shutdown().await;
}

/// Answers `reason` and closes `ch`.
async fn refuse(mut ch: Channel, reason: String) {
    let _ = ops::write_line(&mut ch, &Reply::Refused { reason }).await;
    let _ = ch.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(line: &str) -> Vec<String> {
        line.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn rsyncs_own_receiving_argv_is_accepted() {
        let got = server_options(
            &argv("rsync --server -logDtpre.iLsfxCIvu --delete . moor/42/"),
            "moor/42",
        );
        assert_eq!(got.unwrap(), ["-logDtpre.iLsfxCIvu", "--delete"]);
    }

    #[test]
    fn anything_but_a_receiving_server_into_the_box_dir_is_refused() {
        for bad in [
            "sh -c true",
            "rsync -logDtpre.iLsfxCIvu . moor/42/",
            "rsync --server --sender -logDtpre.iLsfxCIvu . moor/42/",
            "rsync --server -logDtpre.iLsfxCIvu . moor/42/../43",
            "rsync --server -logDtpre.iLsfxCIvu . ../42",
            "rsync --server -logDtpre.iLsfxCIvu . /etc",
            "rsync --server -T /etc -logDtpre.iLsfxCIvu . moor/42/",
            "rsync --server -logDtprT . moor/42/",
            "rsync --server --log-file=/tmp/x -logDtpre.iLsfxCIvu . moor/42/",
            "rsync --server -logDtpre.iLsfxCIvu . moor/42/ moor/43/",
        ] {
            assert!(server_options(&argv(bad), "moor/42").is_err(), "{bad}");
        }
    }

    #[test]
    fn a_pull_reads_only_inside_the_box_dirs() {
        let home = tempfile::tempdir().unwrap();
        let (_, dir, state) = dirs(home.path(), 42);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(dir.join("a.txt"), "ok").unwrap();
        std::fs::write(state.join("s.txt"), "st").unwrap();
        std::fs::write(home.path().join("secret"), "no").unwrap();
        std::os::unix::fs::symlink(home.path().join("secret"), dir.join("out")).unwrap();
        assert_eq!(pulled(home.path(), 42, "a.txt").unwrap().1, 2);
        let in_state = state.join("s.txt").display().to_string();
        assert!(pulled(home.path(), 42, &in_state).is_ok());
        let secret = home.path().join("secret").display().to_string();
        for bad in ["../../secret", secret.as_str(), "out", "."] {
            assert!(pulled(home.path(), 42, bad).is_err(), "{bad}");
        }
    }
}
