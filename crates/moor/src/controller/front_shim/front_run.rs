//! `moor front-run`: a front the controller does not start, given the socket the shim's pid 1
//! gives a front it starts.

#[cfg(target_os = "linux")]
use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::process::Command;

/// `moor front-run --dir DIR -- COMMAND...`: runs COMMAND with the front variables for front
/// directory DIR and exits with its status.
#[cfg(target_os = "linux")]
pub fn run(args: &[OsString]) -> Result<u8, String> {
    const USAGE: &str = "usage: moor front-run --dir DIR -- COMMAND...";
    let mut parts = args.splitn(2, |a| a == "--");
    let (flags, command) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    );
    let ([flag, dir], Some((program, args))) = (flags, command.split_first()) else {
        return Err(USAGE.to_owned());
    };
    if flag != "--dir" {
        return Err(USAGE.to_owned());
    }
    let mut front = Command::new(program);
    front.args(args).env(
        "MOOR_FRONT_SOCKET",
        PathBuf::from(dir).join(crate::front_dir::FRONT_SOCKET),
    );
    super::relay(&mut front)
}

/// `moor front-run` reaps children as the shim does, which only Linux builds.
#[cfg(not(target_os = "linux"))]
pub fn run(_args: &[std::ffi::OsString]) -> Result<u8, String> {
    Err("moor front-run needs Linux".to_owned())
}
