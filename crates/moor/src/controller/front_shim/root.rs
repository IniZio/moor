//! The front's root: a fresh tmpfs that holds only what a front needs, bound read-only (the
//! system dirs, a few devices, its program and `read_only` paths), its state dir read-write
//! and a fresh `/proc`. Everything else of the host, such as `/home`, `/var`, `/run` and
//! `/snap`, and the Unix sockets there, does not exist for the front.

use std::fs;
use std::os::fd::{AsRawFd as _, OwnedFd};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use rustix::fs::{open, Mode, OFlags};
use rustix::io::Errno;
use rustix::mount::{mount, mount_bind, mount_remount, unmount, MountFlags, UnmountFlags};
use rustix::process::pivot_root;

/// The host dirs every front sees, read-only, where they exist: the programs, libraries and
/// configuration a front runs with.
pub(in crate::controller) const SYSTEM: [&str; 6] =
    ["/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc"];

/// The devices a front sees.
const DEVICES: [&str; 6] = ["null", "zero", "full", "random", "urandom", "tty"];

/// Where the new root is assembled before `pivot_root`; a tmpfs there hides nothing the front
/// sees, since every source is opened first.
const STAGE: &str = "/tmp";

/// What the front's root holds beyond the system dirs.
#[derive(Default)]
pub(super) struct Layout {
    /// The dir the front may write; its `tmp` becomes the front's `/tmp`.
    pub(super) writable: Option<PathBuf>,
    /// Files and dirs the front may read.
    pub(super) read_only: Vec<PathBuf>,
}

/// A host path the new root shows at the same path, opened before the stage hid it.
struct Shown {
    /// Its path, in the host and in the new root.
    path: PathBuf,
    /// It, opened with `O_PATH`.
    fd: OwnedFd,
    /// Whether the front may write it.
    writable: bool,
}

/// Builds the front's root from `layout`, mounts a fresh `/proc` in it, pivots into it and
/// detaches the host's root. When fd 3 is open, returns its socket reopened through a read-only
/// mount, so the front cannot chmod the socket through it; the socket is opened again by path
/// because a mount of the controller's namespace cannot be bound into this one.
pub(super) fn enter(layout: &Layout) -> Result<Option<OwnedFd>, String> {
    let shown = sources(layout)?;
    let own_tmp = layout.writable.as_deref().map(own_tmp).transpose()?;
    let socket = fs::read_link("/proc/self/fd/3")
        .ok()
        .and_then(|path| open(&path, OFlags::PATH | OFlags::CLOEXEC, Mode::empty()).ok());
    let stage = Path::new(STAGE);
    let flags = MountFlags::NOSUID | MountFlags::NODEV;
    mount("tmpfs", stage, "tmpfs", flags, c"mode=755")
        .map_err(|e| format!("cannot mount the front's root: {e}"))?;
    for dir in SYSTEM {
        system(stage, Path::new(dir))?;
    }
    for item in &shown {
        bind(
            stage,
            &item.path,
            &format!("/proc/self/fd/{}", item.fd.as_raw_fd()),
            item.writable,
        )?;
    }
    let tmp = stage.join("tmp");
    fs::create_dir_all(&tmp).map_err(|e| format!("cannot make /tmp: {e}"))?;
    if let Some(fd) = &own_tmp {
        bind_at(
            &tmp,
            Path::new(&format!("/proc/self/fd/{}", fd.as_raw_fd())),
            true,
        )?;
    }
    devices(stage)?;
    let proc = stage.join("proc");
    fs::create_dir_all(&proc).map_err(|e| format!("cannot make /proc: {e}"))?;
    mount("proc", &proc, "proc", flags | MountFlags::NOEXEC, None)
        .map_err(|e| format!("cannot mount /proc: {e}"))?;
    let socket = socket.map(|fd| read_only_socket(stage, &fd)).transpose()?;
    mount_remount(stage, MountFlags::BIND | MountFlags::RDONLY | flags, "")
        .map_err(|e| format!("cannot make the front's root read-only: {e}"))?;
    std::env::set_current_dir(stage).map_err(|e| format!("cannot enter the front's root: {e}"))?;
    pivot_root(".", ".").map_err(|e| format!("pivot_root: {e}"))?;
    unmount(".", UnmountFlags::DETACH)
        .map_err(|e| format!("cannot detach the host's root: {e}"))?;
    std::env::set_current_dir("/").map_err(|e| format!("cannot move to /: {e}"))?;
    Ok(socket)
}

/// Opens `dir`'s `tmp`, the front's `/tmp`, made when it is missing.
fn own_tmp(dir: &Path) -> Result<OwnedFd, String> {
    if dir.starts_with(STAGE) {
        return Err(format!(
            "the front's state dir {} lies under /tmp, which is the front's own tmp dir",
            dir.display()
        ));
    }
    let tmp = dir.join("tmp");
    fs::create_dir_all(&tmp).map_err(|e| format!("cannot make {}: {e}", tmp.display()))?;
    open(&tmp, OFlags::PATH | OFlags::CLOEXEC, Mode::empty())
        .map_err(|e| format!("cannot open {}: {e}", tmp.display()))
}

/// Opens each path of `layout` outside the [`SYSTEM`] dirs, which show it already, and the
/// file `/etc/resolv.conf` points to when that lies outside them.
fn sources(layout: &Layout) -> Result<Vec<Shown>, String> {
    let resolv = fs::canonicalize("/etc/resolv.conf")
        .ok()
        .filter(|real| !SYSTEM.iter().any(|dir| real.starts_with(dir)));
    let read_only = layout
        .read_only
        .iter()
        .chain(&resolv)
        .filter(|path| !SYSTEM.iter().any(|dir| path.starts_with(dir)))
        .map(|path| (path, false));
    read_only
        .chain(layout.writable.iter().map(|path| (path, true)))
        .map(|(path, writable)| {
            open(path, OFlags::PATH | OFlags::CLOEXEC, Mode::empty())
                .map(|fd| Shown {
                    path: path.clone(),
                    fd,
                    writable,
                })
                .map_err(|e| format!("cannot open {}: {e}", path.display()))
        })
        .collect()
}

/// Shows the host's `dir` in `stage`: a symlink as the same symlink, a dir bound read-only.
fn system(stage: &Path, dir: &Path) -> Result<(), String> {
    let Ok(meta) = fs::symlink_metadata(dir) else {
        return Ok(());
    };
    if meta.file_type().is_symlink() {
        let target =
            fs::read_link(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        return symlink(target, inside(stage, dir))
            .map_err(|e| format!("cannot link {}: {e}", dir.display()));
    }
    bind(stage, dir, &dir.display().to_string(), false)
}

/// A minimal `/dev`: the [`DEVICES`] and the `fd`, `stdin`, `stdout` and `stderr` links.
fn devices(stage: &Path) -> Result<(), String> {
    let dev = stage.join("dev");
    fs::create_dir_all(&dev).map_err(|e| format!("cannot make /dev: {e}"))?;
    let flags = MountFlags::NOSUID | MountFlags::NOEXEC;
    mount("tmpfs", &dev, "tmpfs", flags, c"mode=755")
        .map_err(|e| format!("cannot mount /dev: {e}"))?;
    for name in DEVICES {
        let host = Path::new("/dev").join(name);
        if host.exists() {
            bind_at(&dev.join(name), &host, true)?;
        }
    }
    for (link, target) in [
        ("fd", "/proc/self/fd"),
        ("stdin", "/proc/self/fd/0"),
        ("stdout", "/proc/self/fd/1"),
        ("stderr", "/proc/self/fd/2"),
    ] {
        symlink(target, dev.join(link)).map_err(|e| format!("cannot link /dev/{link}: {e}"))?;
    }
    mount_remount(&dev, MountFlags::BIND | MountFlags::RDONLY | flags, "")
        .map_err(|e| format!("cannot make /dev read-only: {e}"))
}

/// Binds the socket open as `fd` read-only at a scratch path in `stage`, reopens it there and
/// detaches the mount, which the reopened fd keeps alive.
fn read_only_socket(stage: &Path, fd: &OwnedFd) -> Result<OwnedFd, String> {
    let at = stage.join(".socket");
    bind_at(
        &at,
        Path::new(&format!("/proc/self/fd/{}", fd.as_raw_fd())),
        false,
    )?;
    let reopened = open(&at, OFlags::PATH | OFlags::CLOEXEC, Mode::empty())
        .map_err(|e| format!("cannot reopen the front socket: {e}"))?;
    unmount(&at, UnmountFlags::DETACH)
        .map_err(|e| format!("cannot detach the front socket: {e}"))?;
    fs::remove_file(&at).map_err(|e| format!("cannot remove {}: {e}", at.display()))?;
    Ok(reopened)
}

/// `path` as it lies inside `stage`.
fn inside(stage: &Path, path: &Path) -> PathBuf {
    stage.join(path.strip_prefix("/").unwrap_or(path))
}

/// Binds `source` at `path` inside `stage`.
fn bind(stage: &Path, path: &Path, source: &str, writable: bool) -> Result<(), String> {
    bind_at(&inside(stage, path), Path::new(source), writable)
}

/// Binds `source` at `at`, read-only unless `writable`, after making room for it: a dir for a
/// dir, an empty file for anything else.
fn bind_at(at: &Path, source: &Path, writable: bool) -> Result<(), String> {
    let fail = |e: String| format!("cannot show {} at {}: {e}", source.display(), at.display());
    let is_dir = fs::metadata(source)
        .map_err(|e| fail(e.to_string()))?
        .is_dir();
    if is_dir {
        fs::create_dir_all(at).map_err(|e| fail(e.to_string()))?;
    } else {
        if let Some(parent) = at.parent() {
            fs::create_dir_all(parent).map_err(|e| fail(e.to_string()))?;
        }
        fs::File::create(at).map_err(|e| fail(e.to_string()))?;
    }
    let errno = |e: Errno| fail(e.to_string());
    mount_bind(source, at).map_err(errno)?;
    let flags = mounts()?
        .into_iter()
        .rev()
        .find_map(|(point, flags)| (point == at).then_some(flags))
        .unwrap_or(MountFlags::empty());
    let mode = if writable {
        MountFlags::empty()
    } else {
        MountFlags::RDONLY
    };
    mount_remount(at, MountFlags::BIND | mode | flags, "").map_err(errno)
}

/// Each mount point in `/proc/self/mountinfo`, in mount order, with the flags it must keep when
/// remounted: the kernel refuses to change nosuid, nodev, noexec or atime on a mount that came
/// from the parent namespace.
fn mounts() -> Result<Vec<(PathBuf, MountFlags)>, String> {
    let info = fs::read_to_string("/proc/self/mountinfo")
        .map_err(|e| format!("cannot read /proc/self/mountinfo: {e}"))?;
    Ok(info
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(' ').skip(4);
            let point = unescape(fields.next()?);
            Some((point, kept_flags(fields.next()?)))
        })
        .collect())
}

/// The flags in a mountinfo option list such as `rw,nosuid,relatime` that a remount keeps.
fn kept_flags(options: &str) -> MountFlags {
    let mut flags = MountFlags::empty();
    let mut atime = MountFlags::STRICTATIME;
    for option in options.split(',') {
        match option {
            "nosuid" => flags |= MountFlags::NOSUID,
            "nodev" => flags |= MountFlags::NODEV,
            "noexec" => flags |= MountFlags::NOEXEC,
            "nodiratime" => flags |= MountFlags::NODIRATIME,
            "noatime" => atime = MountFlags::NOATIME,
            "relatime" => atime = MountFlags::RELATIME,
            _ => {}
        }
    }
    flags | atime
}

/// A mountinfo path with its octal escapes, such as `\040` for a space, decoded.
fn unescape(field: &str) -> PathBuf {
    use std::os::unix::ffi::OsStringExt as _;
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let octal = bytes
            .get(i + 1..i + 4)
            .filter(|_| b == b'\\')
            .and_then(|d| std::str::from_utf8(d).ok())
            .and_then(|d| u8::from_str_radix(d, 8).ok());
        if let Some(c) = octal {
            out.push(c);
            i += 4;
        } else {
            out.push(b);
            i += 1;
        }
    }
    PathBuf::from(std::ffi::OsString::from_vec(out))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rustix::mount::MountFlags;

    use super::{kept_flags, unescape};

    #[test]
    fn a_remount_keeps_the_locked_flags_and_the_atime_mode() {
        assert_eq!(
            kept_flags("rw,nosuid,nodev,noexec,relatime"),
            MountFlags::NOSUID | MountFlags::NODEV | MountFlags::NOEXEC | MountFlags::RELATIME
        );
        assert_eq!(kept_flags("ro,noatime"), MountFlags::NOATIME);
        assert_eq!(kept_flags("rw"), MountFlags::STRICTATIME);
    }

    #[test]
    fn mountinfo_escapes_are_decoded() {
        assert_eq!(unescape(r"/a\040b\134c"), PathBuf::from(r"/a b\c"));
    }
}
