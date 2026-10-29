//! `moor front-run`: a front the controller does not start, given what the shim's pid 1 gives
//! a front it starts.

#[cfg(target_os = "linux")]
use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::io;
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
    let dir = PathBuf::from(dir);
    let mut front = Command::new(program);
    front.args(args).env(
        "MOOR_FRONT_SOCKET",
        dir.join(crate::front_dir::FRONT_SOCKET),
    );
    let ca = dir.join(crate::front_dir::CA_PEM);
    match fs::read_to_string(&ca) {
        Ok(pem) => {
            let port =
                super::bridge::Bridge::open(&dir.join(crate::front_dir::PROXY_SOCKET))?.serve()?;
            front
                .env(super::bridge::PORT_VAR, port.to_string())
                .env(super::super::front_proxy::CA_PEM_VAR, pem);
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("cannot read {}: {e}", ca.display())),
    }
    super::relay(&mut front)
}

/// `moor front-run` needs the loopback bridge, which only Linux builds.
#[cfg(not(target_os = "linux"))]
pub fn run(_args: &[std::ffi::OsString]) -> Result<u8, String> {
    Err("moor front-run needs Linux".to_owned())
}
