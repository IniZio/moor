//! Whether the background work Claude Code reported is still alive in the Box.

use moor_core::boxscript::quote;

/// Prints `running`, `none` or `unknown`, looking through the process table at `proc`.
///
/// Claude Code points every shell's stdout at an output file of its own, and that shell stays
/// up while what it started runs. Any such process counts, so a stray one keeps a Box up rather
/// than sleeping it. A process table that cannot be read proves nothing.
pub(super) fn script(proc: &str) -> String {
    let proc = quote(proc);
    format!(
        r#"p={proc}
readlink "$p/self/fd/1" >/dev/null 2>&1 || {{ echo unknown; exit 0; }}
for fd in "$p"/[0-9]*/fd/1; do
  case $(readlink "$fd" 2>/dev/null) in */tasks/*.output) echo running; exit 0 ;; esac
done
echo none
"#
    )
}
