#!/usr/bin/env bash
set -euo pipefail
top=$(git rev-parse --show-toplevel)

crate_dir() {
  local root=$1 name=$2 d
  for d in "$root/$name" "$root/crates/$name"; do
    [ -d "$d/src" ] && { echo "$d"; return; }
  done
  for d in "$root"/*/Cargo.toml "$root"/crates/*/Cargo.toml; do
    [ -f "$d" ] && grep -q "^name *= *\"$name\"" "$d" && { dirname "$d"; return; }
  done
  return 1
}

# usage: check <root> <table>; sets globals rules and bad
check() {
  local root=$1 table=$2 mod allowed crate path dir depth base f line target full
  while IFS=$'\t' read -r mod allowed; do
    case "$mod" in ''|'#'*) continue ;; esac
    rules=$((rules + 1))
    crate=${mod%%::*}
    path=${mod#*::}
    base=$(crate_dir "$root" "$crate") || { echo "layering: no crate dir for $crate"; bad=$((bad + 1)); continue; }
    dir="$base/src/${path//::/\/}"
    depth=$(awk -F'::' '{print NF}' <<<"$path")
    while IFS= read -r f; do
      while IFS=' ' read -r line target; do
        full="$crate::$target"
        case "$mod" in "$full"|"$full::"*) continue ;; esac
        case "$full" in "$mod"::*) continue ;; esac
        case ",${allowed// /}," in *",$full,"*) continue ;; esac
        echo "layering: ${f#"$root"/}:$line $mod uses $full"
        bad=$((bad + 1))
      done < <(awk -v d="$depth" '{
        s = $0
        while (match(s, /crate::[A-Za-z_][A-Za-z0-9_]*(::[A-Za-z_][A-Za-z0-9_]*)*/)) {
          p = substr(s, RSTART + 7, RLENGTH - 7)
          n = split(p, a, "::"); if (n > d) n = d
          t = a[1]; for (i = 2; i <= n; i++) t = t "::" a[i]
          print NR, t
          s = substr(s, RSTART + RLENGTH)
        }
      }' "$f")
    done < <({ [ -f "$dir.rs" ] && echo "$dir.rs"; [ -d "$dir" ] && find "$dir" -name '*.rs' | sort; } || true)
  done <"$table"
}

self_test() {
  local root="$top/target/layering-selftest"
  rm -rf "$root"
  mkdir -p "$root/fake/src"
  printf 'pub mod domain;\npub mod adapter;\n' >"$root/fake/src/lib.rs"
  printf 'use crate::adapter::Thing;\n' >"$root/fake/src/domain.rs"
  printf 'pub struct Thing;\n' >"$root/fake/src/adapter.rs"
  printf 'fake::domain\tfake::util\n' >"$root/table.tsv"
  rules=0 bad=0
  check "$root" "$root/table.tsv" >"$root/out.txt"
  if [ "$bad" -ne 1 ] || ! grep -q 'fake::domain uses fake::adapter' "$root/out.txt"; then
    echo "layering: self-test failed to detect violation"
    exit 1
  fi
  printf 'use crate::util::Helper;\nuse crate::domain::Other;\n' >"$root/fake/src/domain.rs"
  rules=0 bad=0
  check "$root" "$root/table.tsv" >"$root/out.txt"
  if [ "$bad" -ne 0 ]; then
    echo "layering: self-test false positive: $(cat "$root/out.txt")"
    exit 1
  fi
  echo "layering: self-test ok"
}

if [ "${1:-}" = "--self-test" ]; then
  self_test
  exit 0
fi

rules=0
bad=0
check "$top" "$top/tools/layering.tsv"
[ "$bad" -eq 0 ] || exit 1
echo "layering: $rules rules ok"
