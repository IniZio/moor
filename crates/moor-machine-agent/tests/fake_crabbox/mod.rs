//! A crabbox stand-in for a pool host's tests: it logs each call, rejects flags crabbox lacks,
#![allow(clippy::unwrap_used, reason = "tests")]
//! and prints words whose proxy command echoes, so a dial gets back what it sends.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The VM's host key in the fake's known-hosts file.
pub(crate) const HOST_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDryg3K/O+WRGSF38Ao5X8lyiZNuEGIJsM6zeRHShanS";

const SCRIPT: &str = r#"#!/bin/sh
echo "$PWD $*" >> "$HOME/crabbox.log"
for a in "$@"; do
  case "$a" in
    --provider|--id|--slug|--json) ;;
    --*) echo "flag provided but not defined: $a" >&2; exit 2 ;;
  esac
done
case "$1" in
  ssh) strict=yes; [ -e "$HOME/loose" ] && strict=no
    echo "ssh '-F' '/dev/null' '-o' 'StrictHostKeyChecking=$strict' '-o' 'UserKnownHostsFile=$HOME/kh' '-o' 'ProxyCommand=cat' 'vm@vm7'" ;;
  list) cat "$HOME/leases.json" 2>/dev/null || echo '[]' ;;
  pause) if [ -e "$HOME/no-pause" ]; then
      echo "provider=$3 does not support pause" >&2; exit 2
    fi ;;
  stop) if [ -e "$HOME/stop-gone" ]; then echo "lease not found" >&2; exit 4; fi ;;
esac
exit 0
"#;

/// Writes the fake into `home` with the VM's known-hosts file; its path.
pub(crate) fn install(home: &Path) -> PathBuf {
    let path = home.join("crabbox");
    std::fs::write(&path, SCRIPT).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(home.join("kh"), format!("vm7 {HOST_KEY}\n")).unwrap();
    path
}

/// Each call the fake took, as `cwd args`.
pub(crate) fn calls(home: &Path) -> String {
    std::fs::read_to_string(home.join("crabbox.log")).unwrap_or_default()
}
