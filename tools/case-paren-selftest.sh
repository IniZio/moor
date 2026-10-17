#!/usr/bin/env bash
# Proves case-paren.sh flags unparenthesised case patterns in Rust string text and accepts parenthesised ones.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# check <name> <expect: flag|ok> ; fixture on stdin
check() {
  cat > "$tmp/f.rs"
  local rc=0 out
  out="$("$root/tools/case-paren.sh" "$tmp/f.rs")" || rc=$?
  if [ "$2" = flag ] && { [ "$rc" -ne 1 ] || ! grep -q '^case-paren: .*f.rs:' <<< "$out"; }; then
    echo "case-paren-selftest: $1 FAILED (expected a finding)"; echo "$out"; exit 1
  fi
  if [ "$2" = ok ] && { [ "$rc" -ne 0 ] || [ "$out" != "case-paren: ok" ]; }; then
    echo "case-paren-selftest: $1 FAILED (expected ok)"; echo "$out"; exit 1
  fi
  echo "case-paren-selftest: $1 ok"
}

check bare-pattern flag <<'EOF2'
const S: &str = r#"
x=$(case "$m" in
  "$h"/*) echo y ;;
esac)
"#;
EOF2
check paren-pattern ok <<'EOF2'
const S: &str = r#"
x=$(case "$m" in
  ("$h"/*) echo y ;;
esac)
"#;
EOF2
check star-and-alternation ok <<'EOF2'
const S: &str = r#"
case "$m" in
  (a|b) echo y ;;
  (*) echo n ;;
esac
"#;
EOF2
check second-arm-bare flag <<'EOF2'
const S: &str = r#"
case "$m" in
  (a) echo y ;;
  *) echo n ;;
esac
"#;
EOF2
check rust-match-arm ok <<'EOF2'
fn f(o: Option<u8>) -> u8 {
    match o {
        Some(x) => x,
        None => 0,
    }
}
EOF2
check same-line-bare flag <<'EOF2'
const S: &str = "x=$(case $m in a) echo y ;; esac)";
EOF2
check same-line-paren ok <<'EOF2'
const S: &str = "x=$(case $m in (a) echo y ;; (*) echo n ;; esac)";
EOF2
check escaped-newline-bare flag <<'EOF2'
const S: &str = "case $m in\n  a) echo y ;;\nesac";
EOF2
check command-with-paren-not-pattern ok <<'EOF2'
const S: &str = r#"
case "$m" in
  (a)
    echo "$(date)"
    ;;
esac
"#;
EOF2
check comment-line ok <<'EOF2'
const S: &str = r#"
case "$m" in
  # a) is a comment
  (a) echo y ;;
esac
"#;
EOF2
