//! The front's view of the filesystem, set up in its new mount namespace: masked files read as
//! `/dev/null`, covered dirs are empty, every mount is read-only, the writable dirs are
//! read-write again and the read-only paths show again through the covers. Read-only mounts refuse chmod, chown, utimes and xattr changes, even
//! through an fd, which the sandbox alone does not.

use std::fs;

use std::os::fd::{AsRawFd as _, OwnedFd};
use std::path::{Path, PathBuf};

use rustix::fs::{open, Mode, OFlags};
use rustix::io::Errno;
use rustix::mount::{mount, mount_bind, mount_remount, MountFlags};

/// A path the front sees through the covers, open before they hid it.
struct Kept<'a> {
    /// Where the front sees it.
    path: &'a Path,
    /// It, opened with `O_PATH`.
    fd: OwnedFd,
    /// Whether the front may write it.
    writable: bool,
}

/// Masks each of `mask` with `/dev/null`, covers each of `cover` with an empty tmpfs, makes
/// every mount read-only, then binds each of `writable` back read-write and each of `read_only`
/// back read-only, at its own path.
pub(super) fn arrange(
    cover: &[PathBuf],
    mask: &[PathBuf],
    writable: &[PathBuf],
    read_only: &[PathBuf],
) -> Result<(), String> {
    let kept = writable
        .iter()
        .map(|path| (path, true))
        .chain(read_only.iter().map(|path| (path, false)))
        .map(|(path, writable)| {
            open(path, OFlags::PATH | OFlags::CLOEXEC, Mode::empty())
                .map(|fd| Kept { path, fd, writable })
                .map_err(|e| format!("cannot open {}: {e}", path.display()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    for file in mask {
        mount_bind("/dev/null", file)
            .map_err(|e| format!("cannot mask {}: {e}", file.display()))?;
    }
    let flags = MountFlags::NOSUID | MountFlags::NODEV | MountFlags::NOEXEC;
    for dir in outermost(cover) {
        mount("tmpfs", dir, "tmpfs", flags, c"mode=755")
            .map_err(|e| format!("cannot cover {}: {e}", dir.display()))?;
        for kept in kept.iter().filter(|k| k.path.starts_with(dir)) {
            make_room(kept)
                .map_err(|e| format!("cannot make room for {}: {e}", kept.path.display()))?;
        }
    }
    read_only_everywhere()?;
    for kept in &kept {
        bind_back(kept)?;
    }
    Ok(())
}

/// Creates the dir, or the empty file, that `kept` is bound over inside a cover.
fn make_room(kept: &Kept<'_>) -> std::io::Result<()> {
    if rustix::fs::fstat(&kept.fd)?.st_mode & 0o170_000 == 0o040_000 {
        return fs::create_dir_all(kept.path);
    }
    if let Some(parent) = kept.path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::File::create(kept.path).map(drop)
}

/// The dirs of `cover` that no other dir of `cover` contains.
fn outermost(cover: &[PathBuf]) -> Vec<&PathBuf> {
    cover
        .iter()
        .filter(|dir| !cover.iter().any(|o| o != *dir && dir.starts_with(o)))
        .collect()
}

/// Remounts every mount this namespace can reach read-only, keeping its other flags. A mount
/// whose path a cover hid, that this user cannot search, or that another mount hides (the
/// kernel answers EINVAL, as for an automount point) is out of the front's reach already, and so
/// is every mount under `/proc`, which pid 1 covers with a fresh `/proc`.
fn read_only_everywhere() -> Result<(), String> {
    for (point, flags) in mounts()?
        .into_iter()
        .filter(|(point, _)| beyond_proc(point))
    {
        match mount_remount(&point, MountFlags::BIND | MountFlags::RDONLY | flags, "") {
            Ok(()) => {}
            Err(e) if [Errno::NOENT, Errno::ACCESS, Errno::INVAL].contains(&e) => {}
            Err(e) => return Err(format!("cannot make {} read-only: {e}", point.display())),
        }
    }
    Ok(())
}

/// Binds `kept` over whatever the front sees at its path, read-write only when it may write it.
fn bind_back(kept: &Kept<'_>) -> Result<(), String> {
    let path = kept.path;
    let fail = |e: Errno| format!("cannot show the front {}: {e}", path.display());
    mount_bind(format!("/proc/self/fd/{}", kept.fd.as_raw_fd()), path).map_err(fail)?;
    let flags = mounts()?
        .into_iter()
        .rev()
        .find_map(|(point, flags)| (point == path).then_some(flags))
        .unwrap_or(MountFlags::empty());
    let mode = if kept.writable {
        MountFlags::empty()
    } else {
        MountFlags::RDONLY
    };
    mount_remount(path, MountFlags::BIND | mode | flags, "").map_err(fail)
}

/// Whether `point` is outside `/proc`, whose mounts pid 1 covers with a fresh `/proc`.
fn beyond_proc(point: &Path) -> bool {
    !point.starts_with("/proc")
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

    use super::{beyond_proc, kept_flags, outermost, unescape};

    #[test]
    fn mounts_under_proc_are_left_to_the_fresh_proc() {
        assert!(!beyond_proc(std::path::Path::new(
            "/proc/sys/fs/binfmt_misc"
        )));
        assert!(!beyond_proc(std::path::Path::new("/proc")));
        assert!(beyond_proc(std::path::Path::new("/procfs")));
        assert!(beyond_proc(std::path::Path::new("/")));
    }

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

    #[test]
    fn a_cover_inside_another_is_left_to_the_outer_one() {
        let cover = [
            PathBuf::from("/tmp"),
            PathBuf::from("/tmp/x/state"),
            PathBuf::from("/run"),
        ];
        assert_eq!(outermost(&cover), [&cover[0], &cover[2]]);
    }
}
