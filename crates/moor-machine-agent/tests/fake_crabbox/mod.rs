//! A crabbox stand-in for a pool host's tests: it logs each call, rejects flags crabbox lacks,
#![allow(clippy::unwrap_used, reason = "tests")]
//! and prints words whose proxy command echoes, so a dial gets back what it sends.

use std::path::{Path, PathBuf};

/// The VM's host key in the fake's known-hosts file.
pub(crate) const HOST_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDryg3K/O+WRGSF38Ao5X8lyiZNuEGIJsM6zeRHShanS";

const SCRIPT: &str = r#"#!/bin/sh
echo "$PWD $*" >> "$HOME/crabbox.log"
[ -n "$CRABBOX_CACHE_VOLUMES" ] && printf '%s %s\n' "$CRABBOX_CACHE_SCOPE" "$CRABBOX_CACHE_VOLUMES" >> "$HOME/cache.env"
echo "$1 ${CRABBOX_CACHE_STORE_MAX_GB-unset}" >> "$HOME/cap.log"
echo "$1 ${CRABBOX_PAUSE_PUBLISH-unset}" >> "$HOME/publish.log"
for a in "$@"; do
  case "$a" in
    --class) [ -e "$HOME/no-class" ] && { echo "unknown class for provider" >&2; exit 2; } ;;
    --provider|--id|--slug|--json|--configured|--name|--mode) ;;
    --*) echo "flag provided but not defined: $a" >&2; exit 2 ;;
  esac
done
case "$1" in
  ssh) [ -e "$HOME/slow-ssh" ] && exec sleep 300
    strict=yes; [ -e "$HOME/loose" ] && strict=no
    echo "ssh '-F' '/dev/null' '-o' 'StrictHostKeyChecking=$strict' '-o' 'UserKnownHostsFile=$HOME/kh' '-o' 'ProxyCommand=cat' 'vm@vm7'" ;;
  providers) cat "$(dirname "$0")/describe.json" 2>/dev/null ;;
  list) cat "$HOME/leases.json" 2>/dev/null || echo '[]' ;;
  doctor) if [ -e "$HOME/doctor-fail" ]; then
      fail="FAIL checkpoints: cloud-hypervisor v52.0-moor.1 is required (found cloud-hypervisor v52.0)"
      echo "ok      git      /usr/bin/git"
      [ -e "$HOME/doctor-config" ] && echo "failed  config   /c.yaml: permissions 0664 want 0600"
      echo "failed  provider provider=fake class=config external provider command failed: exit status 1: ok kvm: fine"
      i=0; while [ $i -lt 40 ]; do echo "ok check-$i: a long line of a check that passed"; i=$((i+1)); done
      echo "$fail"
      echo "doctor: 21 ok, 3 warn, 1 fail: $fail"
      echo "doctor finished"
      echo "$fail" >&2
      echo "doctor found problems" >&2; exit 1
    fi ;;
  checkpoint) case "$2" in
    create) cat "$HOME/create.json" 2>/dev/null \
        || echo '{"id":"chk_1","native":{"bytes":9,"uniqueBytes":5,"parentCheckpointId":"chk_0"}}' ;;
    fork) if [ -e "$HOME/fork-fail" ]; then echo "network exploded" >&2; exit 2; fi
      cat "$HOME/fork.stderr" >&2 2>/dev/null
      cat "$HOME/fork.json" 2>/dev/null || echo '{"slug":"'"$5"'"}' ;;
    delete) if [ -e "$HOME/delete-fail" ]; then echo "provider exploded" >&2; exit 2; fi
      if [ -e "$HOME/delete-gone" ]; then echo "checkpoint $3 not found" >&2; exit 2; fi ;;
  esac ;;
  pause) if [ -e "$HOME/no-pause" ]; then
      echo "provider=$3 does not support pause" >&2; exit 2
    fi ;;
  stop) if [ -e "$HOME/stop-fail" ]; then echo "provider exploded" >&2; exit 2; fi
    if [ -e "$HOME/stop-gone" ]; then echo "lease not found" >&2; exit 4; fi
    if [ -e "$HOME/stop-missing" ]; then echo "lease $5 not found" >&2; exit 1; fi ;;
esac
exit 0
"#;

/// Writes the fake into `home` with the VM's known-hosts file; its path.
pub(crate) fn install(home: &Path) -> PathBuf {
    let path = home.join("crabbox");
    moor_testkit::write_executable(&path, SCRIPT).unwrap();
    std::fs::write(home.join("kh"), format!("vm7 {HOST_KEY}\n")).unwrap();
    path
}

/// Each call the fake took, as `cwd args`.
pub(crate) fn calls(home: &Path) -> String {
    std::fs::read_to_string(home.join("crabbox.log")).unwrap_or_default()
}
