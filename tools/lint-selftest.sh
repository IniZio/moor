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
