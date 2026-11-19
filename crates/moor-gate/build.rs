//! Builds the account page the gate embeds.
//!
//! `src/login/account.rs` embeds `web/dist` at compile time, and that directory is not in git. This
//! script runs `bun` to fill it. Cargo reruns the script only when a page source, the lockfile or
//! `dist/index.html` changes or goes missing, so a build with the page unchanged runs nothing.
//!
//! With no `bun` the build stops and says so. A placeholder page would let a release or an image
//! ship a gate whose portal is empty, with every check green.
//!
//! `MOOR_WEB_PREBUILT=1` skips bun and requires `web/dist/index.html` to exist. The release image
//! sets it, because its bundle comes from a separate Docker stage.

#![allow(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "a build script talks to cargo through stdout and to the person through stderr"
)]

use std::fs::File;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::SystemTime;

/// Everything the page is built from, relative to this crate. A change to any of it rebuilds.
const INPUTS: [&str; 8] = [
    "../../ui",
    "web/src",
    "web/fonts",
    "web/index.html",
    "web/package.json",
    "web/bun.lock",
    "web/vite.config.ts",
    "web/tsconfig.json",
];

/// The file whose absence means the page was never built.
const ENTRY: &str = "web/dist/index.html";

/// Shown when the page cannot be built.
const NO_BUN: &str =
    "moor-gate embeds the account page, which bun builds, and bun is not installed.
Install bun (the version in the Makefile, BUN_VERSION; https://bun.sh), then run cargo again.
To use a bundle built elsewhere, put it in crates/moor-gate/web/dist and set MOOR_WEB_PREBUILT=1.";

fn main() -> ExitCode {
    for input in INPUTS {
        println!("cargo::rerun-if-changed={input}");
    }
    println!("cargo::rerun-if-changed={ENTRY}");
    println!("cargo::rerun-if-env-changed=MOOR_WEB_PREBUILT");
    let built = if std::env::var_os("MOOR_WEB_PREBUILT").is_some_and(|v| v == "1") {
        require_entry()
    } else {
        bun_build()
            .and_then(|()| require_entry())
            .and_then(|()| backdate())
    };
    match built {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

/// Fails unless the page's entry file exists.
fn require_entry() -> Result<(), String> {
    if Path::new(ENTRY).is_file() {
        Ok(())
    } else {
        Err(format!(
            "crates/moor-gate/{ENTRY} is missing, so there is no account page to embed."
        ))
    }
}

/// Cargo calls a path changed when it is newer than the moment this script started, and the page
/// is written after that. Without this the script would rerun on every build. The entry file is
/// given the oldest possible time, so only a later edit or a deletion reruns the script.
fn backdate() -> Result<(), String> {
    File::options()
        .write(true)
        .open(ENTRY)
        .and_then(|f| f.set_modified(SystemTime::UNIX_EPOCH))
        .map_err(|e| format!("could not mark {ENTRY} as built: {e}"))
}

/// Installs the locked packages and builds the page, as `make web` does.
fn bun_build() -> Result<(), String> {
    for args in [&["install", "--frozen-lockfile"][..], &["run", "build"][..]] {
        let status = Command::new("bun")
            .args(args)
            .current_dir("web")
            .status()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    NO_BUN.to_owned()
                } else {
                    format!("could not run bun: {e}")
                }
            })?;
        if !status.success() {
            return Err(format!("bun {} failed ({status})", args.join(" ")));
        }
    }
    Ok(())
}
