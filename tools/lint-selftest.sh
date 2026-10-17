#!/usr/bin/env bash
# Proves the strict lint template rejects what it should and accepts clean code.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
base="${CARGO_TARGET_DIR:-$root/target}"
dir="$base/lint-selftest"
mkdir -p "$dir/src"

{
  printf '[package]\nname = "lint_selftest"\nversion = "0.0.0"\nedition = "2021"\npublish = false\n\n[workspace]\n\n'
  cat "$root/tools/lint-template/lints.toml"
} > "$dir/Cargo.toml"
cp "$root/clippy.toml" "$dir/clippy.toml"

run() { (cd "$dir" && CARGO_TARGET_DIR="$dir/target" cargo clippy --offline -q -- -D warnings 2>&1); }

# check <case> <expect: pass|fail> <needle> ; source on stdin
check() {
  cat > "$dir/src/lib.rs"
  local rc=0 out
  out="$(run)" || rc=$?
  if [ "$2" = pass ] && [ "$rc" -ne 0 ]; then
    echo "lint-selftest: $1 FAILED (expected pass)"; echo "$out"; exit 1
  fi
  if [ "$2" = fail ] && { [ "$rc" -eq 0 ] || ! grep -q "$3" <<< "$out"; }; then
    echo "lint-selftest: $1 FAILED (expected failure mentioning $3)"; echo "$out"; exit 1
  fi
  echo "lint-selftest: $1 ok"
}

# exec_env <case> <expect: pass|fail> ; fake .rs source on stdin, scanned by tools/exec-env.sh
exec_env() {
  local d="$dir/exec-env-case"
  rm -rf "$d"; mkdir -p "$d"
  cat > "$d/fake.rs"
  local rc=0 out
  out="$("$root/tools/exec-env.sh" "$d" 2>&1)" || rc=$?
  if { [ "$2" = pass ] && [ "$rc" -ne 0 ]; } || { [ "$2" = fail ] && [ "$rc" -eq 0 ]; }; then
    echo "lint-selftest: $1 FAILED (expected $2)"; echo "$out"; exit 1
  fi
  echo "lint-selftest: $1 ok"
}

# seatbelt_text <case> <expect: pass|fail> [relpath] ; fake .rs source on stdin, scanned by tools/seatbelt-text.sh
seatbelt_text() {
  local d="$dir/seatbelt-case" rel="${3:-moor/src/fake.rs}"
  rm -rf "$d"; mkdir -p "$d/$(dirname "$rel")"
  cat > "$d/$rel"
  local rc=0 out
  out="$("$root/tools/seatbelt-text.sh" "$d" 2>&1)" || rc=$?
  if { [ "$2" = pass ] && [ "$rc" -ne 0 ]; } || { [ "$2" = fail ] && [ "$rc" -eq 0 ]; }; then
    echo "lint-selftest: $1 FAILED (expected $2)"; echo "$out"; exit 1
  fi
  echo "lint-selftest: $1 ok"
}

exec_env exec-env-shorthand fail <<'RS'
fn f(env: Vec<(String, String)>) {
    let _ = ExecSpec {
        argv: vec![],
        env,
    };
}
RS
exec_env exec-env-chain fail <<'RS'
fn f() {
    let _ = ExecSpec {
        env: exec.vars().into_iter().chain(extra).collect(),
    };
}
RS
exec_env exec-env-multiline fail <<'RS'
fn f() {
    let _ = ExecSpec {
        env: exec
            .vars(),
    };
}
RS
exec_env exec-env-vars pass <<'RS'
fn f() {
    let _ = ExecSpec {
        env: exec.vars(),
    };
}
RS
exec_env exec-env-empty pass <<'RS'
fn f() {
    let _ = ExecSpec {
        env: Vec::new(),
    };
}
RS
exec_env exec-env-oneline-shorthand fail <<'RS'
fn f() { let _ = ExecSpec { argv, env, cwd: None, .. }; }
RS
exec_env exec-env-oneline-cfg fail <<'RS'
fn f() { let _ = ExecSpec { env: cfg.env.clone(), ..base }; }
RS
exec_env exec-env-oneline-empty pass <<'RS'
fn f() { let _ = ExecSpec { argv, env: Vec::new(), .. }; }
RS
exec_env exec-env-oneline-vars pass <<'RS'
fn f() { let _ = ExecSpec { env: exec.vars(), argv }; }
RS
exec_env exec-env-assign fail <<'RS'
fn f() { spec.env = other; }
RS
exec_env exec-env-push fail <<'RS'
fn f() { spec.env.push(kv); }
RS
exec_env exec-env-extend fail <<'RS'
fn f() { spec.env.extend(kvs); }
RS
exec_env exec-env-read pass <<'RS'
fn f() { let n = cfg.env.len(); }
RS
exec_env exec-env-fn-vars fail <<'RS'
fn vars(&self) -> Vec<(String, String)> { Vec::new() }
RS
exec_env exec-env-proxy-name fail <<'RS'
fn f() -> &'static str { "HTTPS_PROXY" }
RS

check E fail "missing-docs" <<'RS'
//! Crate.
pub const X: u32 = 1;
RS
check F fail "unsafe-code" <<'RS'
//! Crate.
/// Zero.
#[must_use]
pub const fn z() -> u32 { unsafe { core::mem::zeroed() } }
RS
check G fail "must_use_candidate" <<'RS'
//! Crate.
/// One.
pub const fn one() -> u32 { 1 }
RS
check H fail "missing_const_for_fn" <<'RS'
//! Crate.
/// One.
#[must_use]
pub fn one() -> u32 { 1 }
RS
check I fail "indexing_slicing" <<'RS'
//! Crate.
/// First.
#[must_use]
pub fn first(v: &[u32]) -> u32 { v[0] }
RS
check J fail "missing_errors_doc" <<'RS'
//! Crate.
/// Parses.
pub fn parse(s: &str) -> Result<u32, std::num::ParseIntError> { s.parse() }
RS
check A pass "" <<'RS'
//! Clean crate.

/// Parses a number.
///
/// # Errors
/// Returns the parse error when `s` is not a number.
pub fn parse(s: &str) -> Result<u32, std::num::ParseIntError> {
    s.parse()
}
RS

check B fail unwrap_used <<'RS'
//! Crate.

/// Parses.
pub fn parse(s: &str) -> u32 {
    s.parse().unwrap()
}
RS

check C fail allow_attributes_without_reason <<'RS'
//! Crate.

/// Parses.
#[allow(clippy::unwrap_used)]
pub fn parse(s: &str) -> u32 {
    s.parse().unwrap()
}
RS

check D fail cognitive_complexity <<'RS'
//! Crate.

/// Branchy.
#[must_use]
pub fn f(a: u32, b: u32) -> u32 {
    let mut n = 0;
    for i in 0..a {
        for j in 0..b {
            if i > j && j > 1 {
                n += 1;
            } else if i == 7 || j == 9 {
                n += 2;
            } else {
                for k in 0..i {
                    if k > j || k == 3 {
                        n += 3;
                    } else if k == 5 && j == 2 {
                        n += 4;
                    }
                }
            }
        }
    }
    if b == 10 {
        n += 10;
    }
    if b == 11 {
        n += 11;
    }
    if b == 12 {
        n += 12;
    }
    if b == 13 {
        n += 13;
    }
    if b == 14 {
        n += 14;
    }
    if b == 15 {
        n += 15;
    }
    n
}
RS
check K fail "unsafe-code" <<'RS'
//! Crate.
/// Zero.
#[must_use]
#[allow(unsafe_code, reason = "x")]
pub const fn z() -> u32 { unsafe { core::mem::zeroed() } }
RS
check L fail "disallowed_methods" <<'RS'
//! Crate.
/// Now.
#[must_use]
pub fn now() -> std::time::Instant { std::time::Instant::now() }
RS

seatbelt_text allow-in-fake fail <<'RS'
const R: &str = "(allow file-read*)";
RS
seatbelt_text allow-at-eol fail <<'RS'
const R: &str = "(allow
file-read*)";
RS
seatbelt_text allow-list-prose pass <<'RS'
// (allow-list applied)
fn f(allow: u8) {}
RS
seatbelt_text key-in-fake fail <<'RS'
const K: &str = "unsafe_macos_seatbelt_rules";
RS
seatbelt_text allow-in-comment fail <<'RS'
// emits (allow network*) somewhere
fn f() {}
RS
seatbelt_text allow-in-tests.rs fail moor/src/tests.rs <<'RS'
const R: &str = "(allow file-read*)";
RS
seatbelt_text seatbelt-clean pass <<'RS'
fn f() {}
RS
seatbelt_text allow-in-mac.rs pass moor/src/adapters/box_agent/sandbox/mac.rs <<'RS'
const R: &str = "(allow file-read*)";
RS
seatbelt_text allow-in-mac-tests pass moor/src/adapters/box_agent/sandbox/mac/tests.rs <<'RS'
const R: &str = "(allow file-read*)";
RS
