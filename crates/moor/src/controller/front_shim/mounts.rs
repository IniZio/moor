//! The front's view of the filesystem, set up in its new mount namespace: masked files read as
//! `/dev/null`, covered dirs are empty, every mount is read-only, and only the writable dirs are
//! read-write again. Read-only mounts refuse chmod, chown, utimes and xattr changes, even
//! through an fd, which the sandbox alone does not.

use std::fs;

use std::os::fd::{AsRawFd as _, OwnedFd};
use std::path::{Path, PathBuf};

use rustix::fs::{open, Mode, OFlags};
use rustix::io::Errno;
use rustix::mount::{mount, mount_bind, mount_remount, MountFlags};

/// Masks each of `mask` with `/dev/null`, covers each of `cover` with an empty tmpfs, makes
/// every mount read-only, then binds each of `writable` back read-write at its own path.
pub(super) fn arrange(
    cover: &[PathBuf],
    mask: &[PathBuf],
    writable: &[PathBuf],
) -> Result<(), String> {
    let kept = writable
        .iter()
        .map(|dir| {
            open(
                dir,
                OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map(|fd| (dir, fd))
            .map_err(|e| format!("cannot open {}: {e}", dir.display()))
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
        for (path, _) in kept.iter().filter(|(path, _)| path.starts_with(dir)) {
            fs::create_dir_all(path)
                .map_err(|e| format!("cannot make room for {}: {e}", path.display()))?;
        }
    }
    read_only_everywhere()?;
    for (path, fd) in &kept {
        writable_again(path, fd)?;
    }
    Ok(())
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
    for (point, flags) in mounts()? {
        if point.starts_with("/proc") {
            continue;
        }
        match mount_remount(&point, MountFlags::BIND | MountFlags::RDONLY | flags, "") {
            Ok(()) => {}
            Err(e) if [Errno::NOENT, Errno::ACCESS, Errno::INVAL].contains(&e) => {}
            Err(e) => return Err(format!("cannot make {} read-only: {e}", point.display())),
        }
    }
    Ok(())
}

/// Binds the dir open as `fd` over its read-only self at `path`, read-write.
fn writable_again(path: &Path, fd: &OwnedFd) -> Result<(), String> {
    let fail = |e: Errno| format!("cannot let the front write {}: {e}", path.display());
    mount_bind(format!("/proc/self/fd/{}", fd.as_raw_fd()), path).map_err(fail)?;
    let flags = mounts()?
        .into_iter()
        .rev()
        .find_map(|(point, flags)| (point == path).then_some(flags))
        .unwrap_or(MountFlags::empty());
    mount_remount(path, MountFlags::BIND | flags, "").map_err(fail)
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

    use super::{kept_flags, outermost, unescape};

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
