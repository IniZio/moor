//! A pseudo-terminal for a command that asked for one.

use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::process::Stdio;

use moor_client::link::boxes::Size;
use rustix::fs::{Mode, OFlags};
use rustix::pty::{grantpt, openpt, ptsname, unlockpt, OpenptFlags};
use rustix::termios::{tcsetwinsize, Winsize};
use tokio::process::Command;

/// A pty of `size`: its master, and its slave for the command's stdio.
pub(super) fn open(size: Size) -> io::Result<(OwnedFd, OwnedFd)> {
    #[cfg(target_os = "linux")]
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)?;
    // macOS's posix_openpt takes no O_CLOEXEC, so it is set right after.
    #[cfg(not(target_os = "linux"))]
    let master = {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
        rustix::io::fcntl_setfd(&master, rustix::io::FdFlags::CLOEXEC)?;
        master
    };
    grantpt(&master)?;
    unlockpt(&master)?;
    let name = ptsname(&master, Vec::new())?;
    let slave = rustix::fs::open(
        name.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    resize(&master, size)?;
    Ok((master, slave))
}

/// Sets the terminal behind `master` to `size`; the kernel then signals its foreground group.
pub(super) fn resize(master: &OwnedFd, size: Size) -> io::Result<()> {
    let ws = Winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    Ok(tcsetwinsize(master, ws)?)
}

/// `carrier` run under `setsid --ctty`, so the pty on its stdin becomes the controlling terminal
/// of a session of its own. Without that the kernel raises no SIGWINCH on a resize, and a hangup
/// reaches nothing.
pub(super) fn controlled(carrier: &Command) -> Command {
    let std = carrier.as_std();
    let mut cmd = Command::new("setsid");
    cmd.arg("--ctty")
        .arg(std.get_program())
        .args(std.get_args());
    for (key, value) in std.get_envs() {
        match value {
            Some(value) => cmd.env(key, value),
            None => cmd.env_remove(key),
        };
    }
    if let Some(dir) = std.get_current_dir() {
        cmd.current_dir(dir);
    }
    cmd
}

/// The slave as three stdio handles for a command.
pub(super) fn stdio(slave: OwnedFd) -> io::Result<[Stdio; 3]> {
    Ok([
        Stdio::from(slave.try_clone()?),
        Stdio::from(slave.try_clone()?),
        Stdio::from(slave),
    ])
}

/// `fd` as a file tokio can read or write.
pub(super) fn file(fd: &OwnedFd) -> io::Result<tokio::fs::File> {
    Ok(tokio::fs::File::from_std(File::from(fd.try_clone()?)))
}
