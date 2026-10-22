//! Copies out of a Box over a fake ssh that runs the remote command on this host, so the
//! Box's files are this host's files.

use std::fs;
use std::io::{self, Cursor, Read};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{Driver, DriverError};
use moor_core::seam::proc::Tool;

use crate::adapters::crabbox::tests::fake_ssh::{handle, lab, Lab};
use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::{Reply, ScriptedProc};

impl Lab {
    /// A directory standing for the Box's disk.
    fn boxdir(&self) -> PathBuf {
        let d = self.dir.path().join("box");
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// A directory on the host side that starts empty.
    fn host(&self) -> PathBuf {
        let d = self.dir.path().join("host");
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn copy(&self, from: &Path, to: &Path) -> Result<(), DriverError> {
        let from = BoxPath::restore(from.to_str().unwrap()).unwrap();
        self.cb.copy_out(&handle(), &from, to)
    }
}

fn mode(p: &Path) -> u32 {
    fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
}

/// Every name under `dir`, hidden ones included.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_copy_rides_the_box_master_and_spawns_no_crabbox_cp() {
    let lab = lab();
    let src = lab.boxdir().join("a.txt");
    fs::write(&src, "alpha").unwrap();
    for n in 0..2 {
        lab.copy(&src, &lab.host().join(format!("a{n}.txt")))
            .unwrap();
    }
    let crabbox = lab.argvs("crabbox");
    assert_eq!(crabbox.len(), 1, "only the target lookup: {crabbox:?}");
    assert_eq!(crabbox.first().unwrap().first().unwrap(), "ssh");
    let ssh = lab.argvs("ssh");
    assert_eq!(ssh.len(), 2);
    for argv in &ssh {
        lab.assert_on_the_master_and_pinned(argv);
    }
}

#[test]
fn bytes_land_exact_in_an_owner_only_file_for_text_binary_and_empty() {
    let lab = lab();
    let binary: Vec<u8> = (0..=255u8).chain([0, 0xff, b'\n', 0x80]).collect();
    let cases: [(&str, &[u8]); 3] = [("t.txt", b"alpha\n"), ("b.bin", &binary), ("empty", b"")];
    for (name, body) in cases {
        let src = lab.boxdir().join(name);
        fs::write(&src, body).unwrap();
        let to = lab.host().join(name);
        lab.copy(&src, &to).unwrap();
        assert_eq!(fs::read(&to).unwrap(), body, "{name}");
        assert_eq!(mode(&to), 0o600, "{name}");
    }
    assert_eq!(listing(&lab.host()), ["b.bin", "empty", "t.txt"]);
}

#[test]
fn names_with_spaces_or_a_leading_dash_are_copied_not_interpreted() {
    let lab = lab();
    for name in ["a b.txt", "-rf", "--version", "-d x"] {
        let src = lab.boxdir().join(name);
        fs::write(&src, name).unwrap();
        let to = lab.host().join(name);
        lab.copy(&src, &to).unwrap();
        assert_eq!(fs::read_to_string(&to).unwrap(), name);
    }
    let dir = lab.boxdir().join("-p q");
    fs::create_dir_all(dir.join("-n")).unwrap();
    fs::write(dir.join("-n/--x y"), "deep").unwrap();
    let to = lab.host().join("dash dir");
    lab.copy(&dir, &to).unwrap();
    assert_eq!(fs::read_to_string(to.join("-n/--x y")).unwrap(), "deep");
}

#[test]
fn a_directory_copies_whole_with_owner_only_modes() {
    let lab = lab();
    let dir = lab.boxdir().join("out");
    fs::create_dir_all(dir.join("sub/empty")).unwrap();
    fs::write(dir.join("top.txt"), "top").unwrap();
    fs::write(dir.join("sub/inner.bin"), [0u8, 1, 2]).unwrap();
    let to = lab.host().join("got");
    lab.copy(&dir, &to).unwrap();
    assert_eq!(fs::read_to_string(to.join("top.txt")).unwrap(), "top");
    assert_eq!(fs::read(to.join("sub/inner.bin")).unwrap(), [0, 1, 2]);
    for d in ["", "sub", "sub/empty"] {
        assert_eq!(mode(&to.join(d)), 0o700, "{d:?}");
    }
    for f in ["top.txt", "sub/inner.bin"] {
        assert_eq!(mode(&to.join(f)), 0o600, "{f}");
    }
    assert_eq!(listing(&lab.host()), ["got"]);
}

#[test]
fn an_existing_directory_destination_receives_the_copy_inside_it() {
    let lab = lab();
    let src = lab.boxdir().join("a.txt");
    fs::write(&src, "alpha").unwrap();
    lab.copy(&src, &lab.host()).unwrap();
    assert_eq!(
        fs::read_to_string(lab.host().join("a.txt")).unwrap(),
        "alpha"
    );
    fs::write(&src, "beta").unwrap();
    lab.copy(&src, &lab.host().join("a.txt")).unwrap();
    assert_eq!(
        fs::read_to_string(lab.host().join("a.txt")).unwrap(),
        "beta"
    );
    assert_eq!(listing(&lab.host()), ["a.txt"]);
}

#[test]
fn a_missing_source_fails_and_leaves_no_destination() {
    let lab = lab();
    let to = lab.host().join("a.txt");
    let got = lab.copy(&lab.boxdir().join("nope"), &to);
    assert!(
        matches!(&got, Err(DriverError::Failed { summary }) if summary.contains("does not exist in the Box")),
        "{got:?}"
    );
    assert!(listing(&lab.host()).is_empty());
}

#[test]
fn a_dropped_connection_fails_and_leaves_no_partial_destination() {
    let lab = lab();
    let src = lab.boxdir().join("big.bin");
    fs::write(&src, vec![b'x'; 1024 * 1024]).unwrap();
    let kept = lab.host().join("kept.bin");
    fs::write(&kept, "old").unwrap();
    let fresh = lab.host().join("fresh.bin");
    fs::write(lab.bin().join("cut"), "300000").unwrap();
    for to in [&kept, &fresh] {
        let got = lab.copy(&src, to);
        assert!(matches!(got, Err(DriverError::Transport { .. })), "{got:?}");
    }
    fs::remove_file(lab.bin().join("cut")).unwrap();
    fs::write(lab.bin().join("drop"), "").unwrap();
    let got = lab.copy(&src, &fresh);
    assert!(matches!(got, Err(DriverError::Transport { .. })), "{got:?}");
    assert_eq!(fs::read_to_string(&kept).unwrap(), "old");
    assert_eq!(listing(&lab.host()), ["kept.bin"]);
}

#[test]
fn a_link_inside_the_source_is_refused_whole() {
    let lab = lab();
    let dir = lab.boxdir().join("out");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a.txt"), "a").unwrap();
    symlink("/etc/passwd", dir.join("z-link")).unwrap();
    for src in [dir.clone(), dir.join("z-link")] {
        let got = lab.copy(&src, &lab.host().join("got"));
        assert!(matches!(got, Err(DriverError::Failed { .. })), "{got:?}");
    }
    assert!(listing(&lab.host()).is_empty());
}

#[test]
fn a_symlink_destination_is_refused() {
    let lab = lab();
    let src = lab.boxdir().join("a.txt");
    fs::write(&src, "alpha").unwrap();
    let aimed = lab.dir.path().join("aimed");
    fs::create_dir_all(&aimed).unwrap();
    symlink(&aimed, lab.host().join("link")).unwrap();
    let got = lab.copy(&src, &lab.host().join("link"));
    assert!(matches!(got, Err(DriverError::Failed { .. })), "{got:?}");
    assert!(listing(&aimed).is_empty());
}

const SSH_LINE: &str = "ssh -o StrictHostKeyChecking=yes -p 2222 user@1.2.3.4\n";
const BIG: u64 = 64 * 1024 * 1024;
const CHECK_EVERY: u64 = 4 * 1024 * 1024;

/// The host directory the lazy archive watches.
static WATCHED: Mutex<Option<PathBuf>> = Mutex::new(None);
/// The most archive bytes pulled that were not yet on disk under [`WATCHED`].
static MAX_LAG: AtomicU64 = AtomicU64::new(0);

/// Bytes of every file under `dir`, hidden ones included.
fn on_disk(dir: &Path) -> u64 {
    fs::read_dir(dir).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .map(|e| match e.metadata() {
                Ok(m) if m.is_dir() => on_disk(&e.path()),
                Ok(m) => m.len(),
                Err(_) => 0,
            })
            .sum()
    })
}

/// A tar archive generated as it is read, measuring how far the disk trails it.
struct Watched {
    inner: Box<dyn Read + Send>,
    pulled: u64,
    next_check: u64,
}

impl Read for Watched {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.pulled += n as u64;
        if self.pulled >= self.next_check {
            self.next_check += CHECK_EVERY;
            let dir = WATCHED.lock().unwrap().clone().unwrap();
            let lag = self.pulled.saturating_sub(on_disk(&dir));
            MAX_LAG.fetch_max(lag, Ordering::SeqCst);
        }
        Ok(n)
    }
}

fn big_archive() -> Box<dyn Read + Send> {
    let mut h = tar::Header::new_gnu();
    h.set_size(BIG);
    h.set_mode(0o644);
    h.set_path("big.bin").unwrap();
    h.set_cksum();
    let pad = vec![0u8; 1024];
    Box::new(Watched {
        inner: Box::new(
            Cursor::new(h.as_bytes().to_vec())
                .chain(io::repeat(b'x').take(BIG))
                .chain(Cursor::new(pad)),
        ),
        pulled: 0,
        next_check: CHECK_EVERY,
    })
}

fn scripted() -> (Arc<ScriptedProc>, Crabbox) {
    let p = Arc::new(ScriptedProc::new());
    let c = Crabbox::fake(p.clone());
    p.on(Tool::Crabbox, &["ssh"], Reply::Ok(SSH_LINE.to_owned()));
    (p, c)
}

#[test]
fn a_large_file_is_written_as_it_streams_not_held_in_memory() {
    let (p, c) = scripted();
    p.on(Tool::Ssh, &[], Reply::Reader(big_archive));
    let host = tempfile::tempdir().unwrap();
    *WATCHED.lock().unwrap() = Some(host.path().to_path_buf());
    let to = host.path().join("big.bin");
    c.copy_out(
        &BoxHandle::new("p/box"),
        &BoxPath::restore("/work/big.bin").unwrap(),
        &to,
    )
    .unwrap();
    assert_eq!(fs::metadata(&to).unwrap().len(), BIG);
    let lag = MAX_LAG.load(Ordering::SeqCst);
    assert!(lag < 1024 * 1024, "{lag} bytes pulled before reaching disk");
}

#[test]
fn a_failure_hides_the_box_stderr() {
    let (p, c) = scripted();
    p.on(
        Tool::Ssh,
        &[],
        Reply::Fail {
            code: 2,
            stdout: String::new(),
            stderr: "secret raw text".to_owned(),
        },
    );
    let host = tempfile::tempdir().unwrap();
    let got = c.copy_out(
        &BoxHandle::new("p/box"),
        &BoxPath::restore("/work/a.txt").unwrap(),
        &host.path().join("a.txt"),
    );
    assert!(matches!(got, Err(DriverError::Failed { .. })), "{got:?}");
    assert!(!format!("{got:?}").contains("secret raw text"), "{got:?}");
    assert!(listing(host.path()).is_empty());
}

#[test]
fn a_directory_copy_never_replaces_a_directory_empty_or_not() {
    let lab = lab();
    let dir = lab.boxdir().join("out");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("new.txt"), "new").unwrap();
    let there = lab.host().join("out");
    fs::create_dir(&there).unwrap();
    let got = lab.copy(&dir, &lab.host());
    assert!(
        matches!(got, Err(DriverError::Failed { .. })),
        "empty: {got:?}"
    );
    assert!(listing(&there).is_empty());
    fs::write(there.join("old.txt"), "old").unwrap();
    let got = lab.copy(&dir, &lab.host());
    assert!(
        matches!(got, Err(DriverError::Failed { .. })),
        "full: {got:?}"
    );
    assert_eq!(listing(&there), ["old.txt"]);
    assert_eq!(listing(&lab.host()), ["out"]);
}

#[test]
fn a_source_with_a_trailing_slash_copies_as_its_last_component() {
    let lab = lab();
    let dir = lab.boxdir().join("out");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a.txt"), "a").unwrap();
    let from = BoxPath::restore(&format!("{}/", dir.display())).unwrap();
    lab.cb.copy_out(&handle(), &from, &lab.host()).unwrap();
    assert_eq!(
        fs::read_to_string(lab.host().join("out/a.txt")).unwrap(),
        "a"
    );
}

#[test]
fn staging_dirs_left_over_an_hour_go_at_the_next_copy_beside_them() {
    let lab = lab();
    lab.cb.clock.sleep(Duration::from_hours(3));
    let host = lab.host();
    let aged = |name: &str, secs: u64| {
        let d = host.join(name);
        fs::create_dir(&d).unwrap();
        fs::write(d.join("part"), "x").unwrap();
        let at = UNIX_EPOCH + Duration::from_secs(secs);
        fs::File::open(&d).unwrap().set_modified(at).unwrap();
    };
    aged(".moor-cp-0000000100000001", 3600);
    aged(".moor-cp-0000000200000002", 2 * 3600 + 1800);
    aged("kept-old", 0);
    let src = lab.boxdir().join("a.txt");
    fs::write(&src, "alpha").unwrap();
    lab.copy(&src, &host.join("a.txt")).unwrap();
    assert_eq!(
        listing(&host),
        [".moor-cp-0000000200000002", "a.txt", "kept-old"]
    );
}

/// An archive entry: a name, and a body or `None` for a directory.
type Entry<'a> = (&'a str, Option<&'a [u8]>);

/// A tar archive of `entries`, with the names written raw so `..` and absolute names arrive as
/// a hostile Box would send them.
fn archive(entries: &[Entry<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, body) in entries {
        let mut h = tar::Header::new_old();
        assert!(name.len() < 100, "{name}");
        let field = h.as_old_mut().name.get_mut(..name.len()).unwrap();
        field.copy_from_slice(name.as_bytes());
        let (kind, bytes) = body.map_or_else(
            || (tar::EntryType::Directory, &[][..]),
            |b| (tar::EntryType::Regular, b),
        );
        h.set_entry_type(kind);
        h.set_size(bytes.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        out.extend_from_slice(h.as_bytes());
        out.extend_from_slice(bytes);
        out.resize(out.len().next_multiple_of(512), 0);
    }
    out.extend([0u8; 1024]);
    out
}

/// Copies `from` out of a scripted Box whose ssh sends `bytes`, into a fresh host dir.
fn copy_archive(from: &str, bytes: Vec<u8>, host: &Path) -> Result<(), DriverError> {
    let (p, c) = scripted();
    p.on(Tool::Ssh, &[], Reply::Bytes(bytes));
    c.copy_out(
        &BoxHandle::new("p/box"),
        &BoxPath::restore(from).unwrap(),
        host,
    )
}

fn refused_with(got: &Result<(), DriverError>, why: &str) -> bool {
    matches!(got, Err(DriverError::Failed { summary }) if summary.contains(why))
}

#[test]
fn an_archive_naming_a_path_outside_the_source_is_refused_and_writes_nothing_outside() {
    let host = tempfile::tempdir().unwrap();
    let to = host.path().join("to");
    fs::create_dir(&to).unwrap();
    let absolute = format!("{}/abs", host.path().display());
    let cases: [&[Entry<'_>]; 4] = [
        &[("out", None), ("out/../../dotdot", Some(b"x"))],
        &[("out", None), ("out/a", Some(b"a")), ("sneaky", Some(b"x"))],
        &[("out", None), (&absolute, Some(b"x"))],
        &[("../up", Some(b"x"))],
    ];
    for entries in cases {
        let got = copy_archive("/work/out", archive(entries), &to);
        assert!(
            refused_with(&got, "outside the source"),
            "{entries:?}: {got:?}"
        );
        assert_eq!(listing(host.path()), ["to"], "{entries:?}");
        assert!(listing(&to).is_empty(), "{entries:?}");
    }
}

#[test]
fn an_archive_naming_one_file_twice_is_refused() {
    let host = tempfile::tempdir().unwrap();
    let entries: &[Entry<'_>] = &[("a.txt", Some(b"one")), ("a.txt", Some(b"two"))];
    let got = copy_archive("/work/a.txt", archive(entries), host.path());
    assert!(refused_with(&got, "cannot write the copy"), "{got:?}");
    assert!(listing(host.path()).is_empty());
}

#[test]
fn an_empty_archive_is_refused() {
    let host = tempfile::tempdir().unwrap();
    let got = copy_archive("/work/a.txt", archive(&[]), &host.path().join("a.txt"));
    assert!(refused_with(&got, "arrived empty"), "{got:?}");
    assert!(listing(host.path()).is_empty());
}

#[test]
fn a_box_without_tar_is_unsupported() {
    let (p, c) = scripted();
    p.on(
        Tool::Ssh,
        &[],
        Reply::Fail {
            code: 45,
            stdout: String::new(),
            stderr: String::new(),
        },
    );
    let host = tempfile::tempdir().unwrap();
    let got = c.copy_out(
        &BoxHandle::new("p/box"),
        &BoxPath::restore("/work/a.txt").unwrap(),
        &host.path().join("a.txt"),
    );
    assert_eq!(got, Err(DriverError::Unsupported("tar on the Box")));
}
