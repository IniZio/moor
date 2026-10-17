//! Box release scripts for simulator devices, the Box root and the final check.
//!
//! These scripts run on the Mac host outside the sandbox. They only ever address the Task's own
//! device set with `xcrun simctl --set <sim>`; they never shut down or delete devices of other
//! Tasks.

use crate::boxscript::quote;
use crate::task_key::MacLayout;

#[cfg(test)]
mod tests;

/// sh function `udids` printing the UDID of each device in the set `$sim`.
///
/// A device line ends in `(<UUID>) (<State>)`. The UUID is matched as 8-4-4-4-12 uppercase hex
/// anchored between parentheses, and the state must close the line, so a device name that
/// contains UUID-like text is not taken for the UDID.
const UDIDS_FN: &str = "udids() {\n  xcrun simctl --set \"$sim\" list devices 2>/dev/null | sed -n -E 's/^.*\\(([0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12})\\) \\([A-Za-z]+( [A-Za-z]+)*\\) *$/\\1/p'\n}\n";

/// POSIX sh text that deletes every simulator device of the Task and its device logs.
///
/// Each device in `xcrun simctl --set <sim> list devices` is shut down (an already shut down
/// device is not a failure), deleted, then `$HOME/Library/Logs/CoreSimulator/<udid>` is removed
/// with `rm -rf`. `$HOME` is read when the script runs: it is the real login home, and device
/// logs there survive `simctl delete`. When `$HOME` is empty or not absolute the script prints
/// `failed logs: HOME is not an absolute path`, skips the log removal, and still shuts down and
/// deletes the devices. A missing sim directory is success. The script goes on after one step
/// fails, prints `failed <udid>: <what>` for each, and exits 1 at the end when any step failed.
/// It never runs `shutdown booted` or `delete all`.
#[must_use]
pub fn delete_devices_script(layout: &MacLayout) -> String {
    let mut s = format!("sim={}\nrc=0\n", quote(layout.sim.as_str()));
    s.push_str("[ -d \"$sim\" ] || exit 0\n");
    s.push_str(UDIDS_FN);
    s.push_str(
        "one_line() { printf '%s' \"$1\" | tr '\\n' ' '; }\n\
logs=\n\
case \"$HOME\" in\n\
  (/?*) logs=\"$HOME/Library/Logs/CoreSimulator\" ;;\n\
  (*) echo 'failed logs: HOME is not an absolute path'; rc=1 ;;\n\
esac\n\
for u in $(udids); do\n\
  xcrun simctl --set \"$sim\" shutdown \"$u\" >/dev/null 2>&1\n\
  if ! out=$(xcrun simctl --set \"$sim\" delete \"$u\" 2>&1); then\n\
    echo \"failed $u: delete: $(one_line \"$out\")\"; rc=1\n\
  fi\n\
  if [ -n \"$logs\" ] && ! out=$(rm -rf \"$logs/$u\" 2>&1); then\n\
    echo \"failed $u: remove logs: $(one_line \"$out\")\"; rc=1\n\
  fi\n\
done\n\
exit $rc\n",
    );
    s
}

/// POSIX sh text that removes the Box root with `rm -rf`.
///
/// When the sim directory exists, every device in the set is shut down first, and while any
/// device remains in the set the script removes nothing: it prints `left: device <udid>` for
/// each and exits 1. Otherwise success means the root is gone afterwards; a root that is already
/// gone is success. When removal fails and root-owned mounts exist under `<root>/home`, the
/// script prints `left: mount <path>` for each and exits 1. It never unmounts. When removal
/// fails for another reason it prints `failed root: <what>` and exits 1. It never runs
/// `shutdown booted` or `delete all`.
#[must_use]
pub fn remove_root_script(layout: &MacLayout) -> String {
    let mut s = format!(
        "sim={}\nroot={}\nhome={}\n",
        quote(layout.sim.as_str()),
        quote(layout.root.as_str()),
        quote(layout.home.as_str()),
    );
    s.push_str(UDIDS_FN);
    s.push_str(
        "[ -e \"$root\" ] || [ -L \"$root\" ] || exit 0\n\
if [ -d \"$sim\" ]; then\n\
  for u in $(udids); do xcrun simctl --set \"$sim\" shutdown \"$u\" >/dev/null 2>&1; done\n\
  left=0\n\
  for u in $(udids); do echo \"left: device $u\"; left=1; done\n\
  [ \"$left\" = 0 ] || exit 1\n\
fi\n\
out=$(rm -rf \"$root\" 2>&1)\n\
{ [ -e \"$root\" ] || [ -L \"$root\" ]; } || exit 0\n\
mounts=$(mount 2>/dev/null | sed -E 's/^.* on (\\/.*)$/\\1/; s/ type [^ ]+ \\(.*$//; s/ \\(.*$//' | while IFS= read -r m; do\n\
  case \"$m\" in (\"$home\"/*) printf '%s\\n' \"$m\" ;; esac\n\
done)\n\
if [ -n \"$mounts\" ]; then\n\
  printf '%s\\n' \"$mounts\" | while IFS= read -r m; do echo \"left: mount $m\"; done\n\
  exit 1\n\
fi\n\
echo \"failed root: $(printf '%s' \"$out\" | tr '\\n' ' ')\"\n\
exit 1\n",
    );
    s
}

/// POSIX sh text for the final check after release.
///
/// `tagged_pids` and `path_pids` are sh snippets from the caller; each prints one pid per line.
/// The script takes the process snapshot they read before it starts any subshell.
/// The script prints `left: process <pid>` once per pid either prints, `left: root <path>` when
/// the Box root exists, and `left: device <udid>` per device still in the Task's set. It prints
/// `warn: ibtoold <pid> ppid 1` for every ibtoold whose parent is pid 1, of any Task; those
/// warnings do not count as left. Exit status is 0 when no `left:` line was printed, else 1.
#[must_use]
pub fn verify_script(layout: &MacLayout, tagged_pids: &str, path_pids: &str) -> String {
    let mut s = format!(
        "sim={}\nroot={}\nrc=0\n",
        quote(layout.sim.as_str()),
        quote(layout.root.as_str()),
    );
    s.push_str(&super::release::snapshot_cmd());
    s.push_str(UDIDS_FN);
    s.push_str("pids() {\n  (\n:\n");
    s.push_str(tagged_pids);
    s.push_str("\n  ) 2>/dev/null\n  (\n:\n");
    s.push_str(path_pids);
    s.push_str("\n  ) 2>/dev/null\n}\n");
    s.push_str(
        "for p in $(pids | sort -u); do echo \"left: process $p\"; rc=1; done\n\
if [ -e \"$root\" ] || [ -L \"$root\" ]; then echo \"left: root $root\"; rc=1; fi\n\
if [ -d \"$sim\" ]; then\n\
  for u in $(udids); do echo \"left: device $u\"; rc=1; done\n\
fi\n\
ps -axo pid=,ppid=,comm= 2>/dev/null | awk '$2 == 1 && $3 ~ /(^|\\/)ibtoold$/ { print \"warn: ibtoold \" $1 \" ppid 1\" }'\n\
exit $rc\n",
    );
    s
}

/// What [`verify_script`] reported.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VerifyReport {
    /// Text after `left: ` of each `left:` line, in output order.
    pub left: Vec<String>,
    /// Text after `warn: ` of each `warn:` line, in output order.
    pub warnings: Vec<String>,
}

/// Reads the `left:` and `warn:` lines of [`verify_script`] output.
///
/// Only lines that start with the prefix count; other lines are ignored.
#[must_use]
pub fn verify_report(stdout: &str) -> VerifyReport {
    let mut report = VerifyReport::default();
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("left: ") {
            report.left.push(rest.to_owned());
        } else if let Some(rest) = line.strip_prefix("warn: ") {
            report.warnings.push(rest.to_owned());
        }
    }
    report
}

/// Reads the `failed <udid>: <what>` lines of [`delete_devices_script`] output.
///
/// Returns the text after `failed `; other lines are ignored.
#[must_use]
pub fn failed_lines(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|l| l.strip_prefix("failed "))
        .map(str::to_owned)
        .collect()
}
