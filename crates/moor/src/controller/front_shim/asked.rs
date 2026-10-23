//! What `moor front-shim` was asked, read from its flags.

use std::ffi::OsString;
use std::path::PathBuf;

use rustix::process::Pid;

use super::Layout;

/// What `moor front-shim` was asked: the controller it serves and the front's root.
pub(super) struct Asked {
    /// The controller that started this shim; pid 1 is not told.
    pub(super) parent: Option<Pid>,
    /// The front Proxy's socket, which pid 1 bridges to a loopback port.
    #[cfg(target_os = "linux")]
    pub(super) proxy: Option<PathBuf>,
    /// What the front's root holds.
    pub(super) layout: Layout,
}

impl Asked {
    /// Reads `--writable DIR` and, on Linux, `--proxy SOCKET` (at most one each) and
    /// `--read-only PATH` flags, and `--parent PID` when `outer`.
    pub(super) fn parse(flags: &[OsString], outer: bool) -> Result<Self, String> {
        let mut asked = Self {
            parent: None,
            #[cfg(target_os = "linux")]
            proxy: None,
            layout: Layout::default(),
        };
        let mut flags = flags.iter();
        while let Some(flag) = flags.next() {
            let word = flags
                .next()
                .ok_or_else(|| format!("{} needs a value", flag.display()))?;
            match flag.to_str() {
                Some("--parent") if outer => {
                    asked.parent = word
                        .to_str()
                        .and_then(|w| w.parse().ok())
                        .and_then(Pid::from_raw);
                    if asked.parent.is_none() {
                        return Err(format!("--parent {} is not a pid", word.display()));
                    }
                }
                #[cfg(target_os = "linux")]
                Some("--proxy") if asked.proxy.is_none() => {
                    asked.proxy = Some(PathBuf::from(word));
                }
                Some("--writable") if asked.layout.writable.is_none() => {
                    asked.layout.writable = Some(PathBuf::from(word));
                }
                Some("--read-only") => asked.layout.read_only.push(PathBuf::from(word)),
                _ => return Err(format!("unknown flag {}", flag.display())),
            }
        }
        Ok(asked)
    }
}
