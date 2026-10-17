#!/usr/bin/env bash
# Rejects shell `case` patterns without a leading `(` in shell text embedded in Rust sources.
# macOS /bin/sh (bash 3.2) misreads `pat)` inside `$( ... )` ("syntax error near unexpected token ';;'"); Linux shells accept it, so only a structural check catches it.
#
# Usage: case-paren.sh [file.rs ...]   (default: every tracked crates/**/*.rs)
#
# Heuristic, line based, deliberately over-eager (a false positive can be explained, a miss cannot):
#  - a `case <word> in` (word holds a `$` or quote, so prose like "case blocks in" is ignored) anywhere in a non-`//` line opens a case; Rust `match` arms never contain that keyword shape.
#  - the text is cut into segments at `;;` and at a literal `\n` escape (one-line format strings).
#  - the segment after `in`, and each segment after `;;`, is a pattern segment. Leading whitespace and a Rust
#    string opener (`"` or `r#"`) are ignored; blank and `#` comment segments are skipped.
#  - a pattern segment containing `)` must start with `(`, otherwise it is reported. One without `)` is a
#    multi-line pattern and the next segment is checked instead.
#  - `esac` closes a case; nested cases are counted. Command segments between patterns are not checked.
# Known false positives: prose "case x in" inside a string, and unbalanced `esac` bookkeeping in odd templates.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

if [ "$#" -gt 0 ]; then files=("$@"); else mapfile -t files < <(git ls-files 'crates/*.rs'); fi

awk '
function trim(s) { sub(/^[ \t]+/, "", s); sub(/[ \t]+$/, "", s); return s }
function opener(s) { s = trim(s); if (match(s, /^r#*"/)) s = substr(s, RLENGTH + 1); else if (substr(s, 1, 1) == "\"") s = substr(s, 2); return trim(s) }
# One segment; expect=1 means it should start with a pattern.
function seg(s,   t) {
  t = opener(s)
  if (expect && t != "" && substr(t, 1, 1) != "#" && t !~ /^esac([^A-Za-z0-9_]|$)/ && index(t, ")") > 0) {
    if (substr(t, 1, 1) != "(" && key != FILENAME ":" FNR) { key = FILENAME ":" FNR; print "case-paren: " FILENAME ":" FNR " " trim(line); bad = 1 }
    expect = 0
  }
  while (depth > 0 && match(t, /(^|[; \t])esac([^A-Za-z0-9_]|$)/)) { depth--; expect = 0; t = substr(t, RSTART + RLENGTH) }
}
FNR == 1 { depth = 0; expect = 0 }
{
  line = $0
  if (line ~ /^[ \t]*\/\//) next
  rest = line
  if (match(rest, /(^|[^A-Za-z0-9_$])case[ \t]+("[^"]*"|[^ \t;]*[$"][^ \t;]*)[ \t]+in([ \t]|$|\\n)/)) {
    depth++; expect = 1
    rest = substr(rest, RSTART + RLENGTH)
  }
  n = split(rest, parts, /;;|\\n/)
  for (i = 1; i <= n; i++) { seg(parts[i]); if (i < n && depth > 0) expect = 1 }
}
END { exit bad }
' "${files[@]}" && echo "case-paren: ok" || exit 1
