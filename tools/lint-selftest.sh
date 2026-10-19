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

# removed_names <case> <expect: pass|fail> ; fake .rs source on stdin, scanned by tools/removed-names.sh
removed_names() {
  local d="$dir/removed-names-case"
  rm -rf "$d"; mkdir -p "$d/crates"
  cat > "$d/crates/fake.rs"
  local rc=0 out
  out="$(MOOR_SCAN_ROOT="$d" "$root/tools/removed-names.sh" 2>&1)" || rc=$?
  if { [ "$2" = pass ] && [ "$rc" -ne 0 ]; } || { [ "$2" = fail ] && [ "$rc" -eq 0 ]; }; then
    echo "lint-selftest: $1 FAILED (expected $2)"; echo "$out"; exit 1
  fi
  echo "lint-selftest: $1 ok"
}

removed_names removed-names-type fail <<'RS'
struct NewTask;
RS
removed_names removed-names-ident fail <<'RS'
fn f(task_input: u8) {}
RS
removed_names removed-names-allowed pass <<'RS'
const S: &str = "2 background tasks running";
fn g(w: &[&str]) -> bool { matches!(w, [n, "background", "task"]) }
RS
removed_names removed-names-clean pass <<'RS'
fn f(job: u8) {}
RS

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

# Lints that scan the tree through git must fail loudly in a tree that is not a git checkout (a `git archive` export).
# The list is fixed on purpose: a new tree-scanning tools/*.sh must be added here and source lib/repo-root.sh.
# stack-overlap.sh is excluded: it compares refs and PR heads, never scans working-tree files, and already dies on a bad ref.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE
export_dir="$(mktemp -d)"
trap 'rm -rf "$export_dir"' EXIT
mkdir -p "$export_dir/tools" "$export_dir/crates"
cp -r "$root/tools/lib" "$export_dir/tools/"
echo 'fn main() {}' > "$export_dir/crates/a.rs"
for name in file-cap.sh removed-names.sh exec-env.sh; do
  script="$root/tools/$name"
  if ! grep -q 'lib/repo-root.sh' "$script"; then
    echo "lint-selftest: $name FAILED (does not source lib/repo-root.sh)"; exit 1
  fi
  cp "$script" "$export_dir/tools/$name"
  rc=0
  out="$(cd "$export_dir" && GIT_CEILING_DIRECTORIES="$(dirname "$export_dir")" "tools/$name" 2>&1)" || rc=$?
  if [ "$rc" -eq 0 ] || ! grep -q 'not a git checkout' <<< "$out"; then
    echo "lint-selftest: $name outside git FAILED (expected non-zero 'not a git checkout')"; echo "$out"; exit 1
  fi
  echo "lint-selftest: $name outside git ok"
done
