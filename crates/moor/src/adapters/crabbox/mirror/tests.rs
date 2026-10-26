//! Mirrors a Box over a fake ssh that runs the Box's rsync on this host, so the Box's files are
//! this host's files.

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::name::BoxPath;
use moor_core::seam::driver::{Driver, DriverError, MirrorStats};

use crate::adapters::crabbox::tests::fake_ssh::{handle, lab, tools, Lab};
use crate::adapters::crabbox::Crabbox;
use crate::adapters::proc::SystemProc;
use crate::env::Env;
use crate::testing::clock::FakeClock;
use crate::testing::rng::FakeRng;

/// Records its argv in `ssh.<n>`, drops the ssh words up to the Box's destination, and runs
/// the rest as the remote command under sh, as sshd would. A `no-rsync` file leaves the remote
/// `PATH` without rsync. `ssh -G` goes to the real ssh.
const RSYNC_SSH: &str = r#"#!/bin/sh
d=$(dirname "$0")
if test "$1" = -G; then exec /usr/bin/ssh "$@"; fi
printf '%s\0' "$@" > "$d/ssh.$(ls "$d" | grep -c '^ssh\.')"
p="$d/hostguard:/usr/bin:/bin"
if test -f "$d/no-rsync"; then p="$d/hostguard"; fi
while test $# -gt 0; do
  case "$1" in box|u@box) shift; break ;; esac
  shift
done
PATH="$p" HOME="$d/home" exec /bin/sh -c "$*"
"#;

fn rsync_lab() -> Lab {
    let lab = lab();
    moor_testkit::write_executable(&lab.bin().join("ssh"), RSYNC_SSH).unwrap();
    lab
}

impl Lab {
    fn tree(&self) -> PathBuf {
        let d = self.dir.path().join("box/work");
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn dest(&self) -> PathBuf {
        self.dir.path().join("host/mirror")
    }

    fn mirror(&self) -> Result<MirrorStats, DriverError> {
        mirror_with(&self.cb, &self.tree(), &self.dest())
    }
}

fn mirror_with(cb: &Crabbox, from: &Path, to: &Path) -> Result<MirrorStats, DriverError> {
    let from = BoxPath::restore(from.to_str().unwrap()).unwrap();
    cb.mirror_out(&handle(), &from, to)
}

fn seed(tree: &Path) {
    fs::create_dir_all(tree.join("src/deep")).unwrap();
    fs::write(tree.join("src/deep/a.txt"), "alpha").unwrap();
    fs::write(tree.join("run.sh"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(tree.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    symlink("src/deep/a.txt", tree.join("link")).unwrap();
    fs::create_dir_all(tree.join(".git")).unwrap();
    fs::write(tree.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
}

#[test]
fn a_mirror_keeps_links_modes_and_nested_dirs_and_leaves_git_out() {
    let lab = rsync_lab();
    seed(&lab.tree());
    let stats = lab.mirror().unwrap();
    let dest = lab.dest();
    assert_eq!(stats.files, 2, "{stats:?}");
    assert_eq!(
        fs::read_to_string(dest.join("src/deep/a.txt")).unwrap(),
        "alpha"
    );
    let link = fs::symlink_metadata(dest.join("link")).unwrap();
    assert!(link.file_type().is_symlink(), "the link arrives as a link");
    assert_eq!(
        fs::read_link(dest.join("link")).unwrap(),
        Path::new("src/deep/a.txt")
    );
    let mode = fs::metadata(dest.join("run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755, "the executable keeps +x");
    assert!(!dest.join(".git").exists(), ".git is left out");
}

#[test]
fn a_second_mirror_sends_only_the_changed_file_and_drops_deleted_ones() {
    let lab = rsync_lab();
    let tree = lab.tree();
    seed(&tree);
    lab.mirror().unwrap();
    fs::write(tree.join("src/deep/a.txt"), "alpha, changed").unwrap();
    fs::remove_file(tree.join("run.sh")).unwrap();
    let stats = lab.mirror().unwrap();
    assert_eq!(stats.files, 1, "{stats:?}");
    let dest = lab.dest();
    assert_eq!(
        fs::read_to_string(dest.join("src/deep/a.txt")).unwrap(),
        "alpha, changed"
    );
    assert!(
        !dest.join("run.sh").exists(),
        "a file gone from the Box goes from the host"
    );
}

#[test]
fn a_mirror_rides_the_box_master_and_keeps_the_pinning() {
    let lab = rsync_lab();
    seed(&lab.tree());
    lab.mirror().unwrap();
    let ssh = lab.argvs("ssh");
    assert_eq!(ssh.len(), 1, "{ssh:?}");
    lab.assert_on_the_master_and_pinned(ssh.first().unwrap());
}

#[test]
fn a_box_without_rsync_is_named() {
    let lab = rsync_lab();
    seed(&lab.tree());
    fs::write(lab.bin().join("no-rsync"), "").unwrap();
    let err = lab.mirror().unwrap_err();
    assert!(
        matches!(err, DriverError::Unsupported("rsync on the Box")),
        "{err:?}"
    );
}

#[test]
fn a_host_without_rsync_is_named() {
    let lab = rsync_lab();
    seed(&lab.tree());
    let env = Env::from_pairs([
        ("PATH", "/usr/bin:/bin"),
        ("MOOR_RSYNC_BIN", "/nonexistent/moor-test/rsync"),
    ]);
    let proc = Arc::new(SystemProc::new(&env, &tools(&lab.bin())));
    let clock = Arc::new(FakeClock::new(moor_core::seam::clock::Ts::from_millis(0)));
    let dir = lab.dir.path().join("other");
    let cb = Crabbox::new(proc, clock, Arc::new(FakeRng::default()))
        .with_state_dir(Some(dir.join("state")), dir.join("s"));
    let _ = cb.version_ok.set(());
    let err = mirror_with(&cb, &lab.tree(), &lab.dest()).unwrap_err();
    assert!(
        matches!(err, DriverError::Unsupported("rsync on the host")),
        "{err:?}"
    );
}
