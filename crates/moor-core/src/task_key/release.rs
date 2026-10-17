//! Box release scripts that stop and kill a Task's processes.
//!
//! The scripts run on the Mac host outside the sandbox, as the same user as the Task's
//! processes, so no OS handle exists. Processes are found by recorded pgid, by the
//! `MOOR_BOX_TAG` value in their environment and by paths under the Task root.

use crate::boxscript::quote;
use crate::name::BoxPath;
use crate::task_key::{MacLayout, TaskKey};

#[cfg(test)]
mod tests;

/// Defines `moor_ps` (one line per process: pid, ppid, pgid, session, command, then the
/// environment) unless the caller already did, `moor_ancestors` (this shell and every ancestor, up
/// to pid 1), `moor_scope` and `moor_confirm` (unless the caller defined it).
///
/// `moor_confirm` reads pids and prints those still alive whose live command line is the start
/// of that pid's command text in `$snap`, so a pid that exited and was reused is dropped.
///
/// `moor_scope` sets `ex` (the ancestors), `sc` (the snapshot column that holds the session id)
/// and `sv` (that column's value for this shell). The session id from `ps -o sess=` is the sid
/// on Linux and a `ps` keyword on macOS. When it is empty or 0, as on macOS builds that hide
/// the kernel session pointer, `sv` stays empty and no session is excluded. No other id stands
/// in for it: the topmost ancestor's process group can be a shared sshd covering other
/// connections, and a Task process must never be hidden by a guess.
const PRELUDE: &str = r#"type moor_ps >/dev/null 2>&1 || moor_ps() { ps -E -ww -ax -o pid=,ppid=,pgid=,sess=,command= 2>/dev/null; }
moor_ancestors() { p=$$; while [ "${p:-0}" -gt 1 ]; do echo "$p"; p=$(ps -o ppid= -p "$p" 2>/dev/null | tr -d ' '); done; }
moor_scope() { ex=$(moor_ancestors); sc=4; sv=$(ps -o sess= -p $$ 2>/dev/null | tr -d ' ')
  [ "$sv" = 0 ] && sv=; }
type moor_confirm >/dev/null 2>&1 || moor_confirm() { while read -r q; do [ -n "$q" ] || continue
  cur=$(ps -ww -p "$q" -o command= 2>/dev/null | LC_ALL=C awk '{$1=$1;print}' 2>/dev/null); [ -n "$cur" ] || continue
  printf '%s\n' "$snap" | Q="$q" CUR="$cur" LC_ALL=C awk '$1==ENVIRON["Q"]{$1=$2=$3=$4="";sub(/^ +/,"");f=(index($0,ENVIRON["CUR"])==1);exit}END{exit !f}' 2>/dev/null && echo "$q"; done; }
"#;

/// sh snippet that defines the helpers and takes the one process snapshot, `snap`, that every
/// matcher reads.
///
/// A caller must run it first, at the top level of its script and before any `(`, `$(` or pipe
/// that runs the matchers, so no process the script forks later is in the snapshot. The only
/// script-made process in it is the `$(moor_ps)` child, which is a descendant of `$$` and is
/// skipped. [`tagged_pids_cmd`], [`path_pids_cmd`] and `select` never run `moor_ps` themselves.
#[must_use]
pub fn snapshot_cmd() -> String {
    format!("{PRELUDE}snap=$(moor_ps)\n")
}

/// Environment `awk` needs to skip the release's own processes.
const SCOPE_ENV: &str = r#"EX="$ex" ME=$$ SC="$sc" SV="$sv""#;

/// Reads the whole snapshot (columns pid, ppid, pgid, session, command, environment) and defines
/// `skip(pid)`, true for the script's ancestors, every descendant of the script at any depth, and
/// every process in the script's session (column `SC` equal to `SV`). The session is not used
/// when `SV` is empty or when every process in the snapshot shares it, since that would skip
/// everything. The matcher goes in `END` and walks `row[1..NR]`.
const SKIP_AWK: &str = r#"BEGIN{n=split(ENVIRON["EX"],a," ");for(i=1;i<=n;i++)s[a[i]]=1;me=ENVIRON["ME"]+0;sc=ENVIRON["SC"]+0;sv=ENVIRON["SV"]}
{par[$1]=$2;ses[$1]=$sc;row[NR]=$0;if($sc!=sv)other=1}
function skip(p,  q,d){if(p in s)return 1;if(sv!=""&&other&&ses[p]==sv)return 1;for(q=par[p];(q+0)>1&&d<512;q=par[q]){if(q+0==me)return 1;d++}return 0}
"#;

/// `moor_alive "<pids>"` is true while any pid lives; `moor_wait "<pids>" <n>` sleeps up to
/// `n` half-seconds until none lives.
const WAIT: &str = r#"moor_alive() { for q in $1; do kill -0 "$q" 2>/dev/null && return 0; done; return 1; }
moor_wait() { i=0; while [ "$i" -lt "$2" ] && moor_alive "$1"; do sleep 0.5; i=$((i+1)); done; }
"#;

/// Whole-set signalling for the kill script, whose cost does not grow per pid: `moor_sig` sends
/// one `kill` per 200 pids, `moor_live` prints which of a set still exist from one `ps`, and
/// `moor_settle "<pids>" <n>` polls the set every 0.2s up to `n` times, leaving the survivors in
/// `live`. No pid is waited on alone.
const SET_OPS: &str = r#"moor_sig() { s=$1; shift; while [ "$#" -gt 0 ]; do n=0; c=
  while [ "$#" -gt 0 ] && [ "$n" -lt 200 ]; do c="$c $1"; shift; n=$((n+1)); done; kill -"$s" $c 2>/dev/null; done; }
moor_live() { [ -n "$1" ] || return 0; ps -o pid= -p "$(printf '%s\n' $1 | paste -sd, -)" 2>/dev/null | tr -d ' '; }
moor_settle() { i=0; live=$(moor_live "$1")
  while [ -n "$live" ] && [ "$i" -lt "$2" ]; do sleep 0.2; i=$((i+1)); live=$(moor_live "$1"); done; }
"#;

/// Skips the script's own session, then keeps lines holding a `MOOR_BOX_TAG=<value>` token whose
/// value matches `PAT`, and whose executable matches `EXE` when `EXE` is set.
const TAG_AWK: &str = r#"BEGIN{pat=ENVIRON["PAT"];exe=ENVIRON["EXE"]}
END{for(r=1;r<=NR;r++){$0=row[r];if(skip($1))continue;if(exe!=""&&$5!~exe)continue
for(i=5;i<=NF;i++)if(index($i,"MOOR_BOX_TAG=")==1&&substr($i,14)~pat){print $1;break}}}"#;

/// Skips the script's own session, then keeps lines whose command text (everything after the
/// session column) matches `PAT`.
const PATH_AWK: &str = r#"BEGIN{pat=ENVIRON["PAT"]}
END{for(r=1;r<=NR;r++){$0=row[r];pid=$1;if(skip(pid))continue;$1=$2=$3=$4="";if($0~pat)print pid}}"#;

/// Keeps ibtoold processes outside the script's session whose parent is pid 1 and prints each
/// pid with 1 when the command line matches `PAT`, else 0.
const IBTOOLD_AWK: &str = r#"END{for(r=1;r<=NR;r++){$0=row[r];pid=$1;if($2!=1||$5!~/(^|\/)ibtoold$/||skip(pid))continue;$1=$2=$3=$4="";print pid,($0~ENVIRON["PAT"])?1:0}}"#;

/// Escapes ERE metacharacters so `s` matches itself literally.
fn ere_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if ".[]{}()*+?^$|\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// ERE for command text that names one of `paths` or anything under it.
///
/// The path must start after a non-path character and end at `/` or a non-path character, so
/// `/w/app` never matches `/w/app-two/x` or `/x/w/app`.
fn path_re(paths: &[&BoxPath]) -> String {
    let alts: Vec<String> = paths.iter().map(|p| ere_escape(p.as_str())).collect();
    format!(
        "(^|[^A-Za-z0-9_./-])({})([^A-Za-z0-9_.-]|$)",
        alts.join("|")
    )
}

/// Runs `awk` over the `$snap` text with the exclusions set, printing matching pids that
/// `moor_confirm` finds unchanged.
fn select(awk: &str, pat: &str, exe: &str) -> String {
    format!(
        "{{ moor_scope; printf '%s\\n' \"$snap\" | {SCOPE_ENV} LC_ALL=C PAT={} EXE={} awk {} 2>/dev/null | moor_confirm; }}",
        quote(pat),
        quote(exe),
        quote(&format!("{SKIP_AWK}{awk}"))
    )
}

/// ERE that matches a `MOOR_BOX_TAG` value of this Task only.
///
/// The whole value must be `<key>-` and 16 lowercase hex digits, so the key `app` never
/// matches the value `app-two-<nonce>`. The key is escaped for regex use.
#[must_use]
pub fn tag_pattern(key: &TaskKey) -> String {
    format!("^{}-[0-9a-f]{{16}}$", ere_escape(key.as_str()))
}

/// sh snippet printing, one per line, pids whose environment holds a `MOOR_BOX_TAG` that
/// matches [`tag_pattern`].
///
/// The script's own pid, every ancestor, every descendant at any depth and every process in its
/// session are never printed. The caller must run [`snapshot_cmd`] first; the snippet reads
/// `$snap`.
#[must_use]
pub fn tagged_pids_cmd(key: &TaskKey) -> String {
    format!("{PRELUDE}{}", select(TAG_AWK, &tag_pattern(key), ""))
}

/// sh snippet printing pids whose command line names a path under `layout.root`.
///
/// The path must be `<root>/...` or exactly `<root>`, so `/w/app` never matches
/// `/w/app-two`. The script's own pid, every ancestor, every descendant and every process in
/// its session are never printed. The caller must run [`snapshot_cmd`] first.
///
/// Release kills ANY process whose arguments name a path under the Task root `<root>/`, including
/// an owner's own ssh or monitor shell sitting in that directory.
#[must_use]
pub fn path_pids_cmd(layout: &MacLayout) -> String {
    format!(
        "{PRELUDE}{}",
        select(PATH_AWK, &path_re(&[&layout.root]), "")
    )
}

/// Keeps lines whose pid is the parent of a pid in `KIDS` and that carry a matching
/// `MOOR_BOX_TAG` token (`TAG`) or whose command text matches `PAT`. The script's ancestors,
/// descendants and session are skipped.
const PARENT_AWK: &str = r#"BEGIN{n=split(ENVIRON["KIDS"],k," ");for(i=1;i<=n;i++)kid[k[i]]=1;tag=ENVIRON["TAG"];pat=ENVIRON["PAT"]}
END{for(r=1;r<=NR;r++){$0=row[r];pid=$1;t=0;for(i=5;i<=NF;i++)if(index($i,"MOOR_BOX_TAG=")==1&&substr($i,14)~tag)t=1;$1=$2=$3=$4="";hit[pid]=(t||$0~pat)&&!skip(pid)}
for(q in kid){p=par[q];if((p+0)>1&&hit[p]&&!(p in o)){o[p]=1;print p}}}"#;

/// TERM the Task's own `nono` (a tagged process whose command is `nono`) and its launch shell,
/// wait up to `wait_secs` for both to exit, then KILL the survivors.
///
/// The launch shell exports `MOOR_BOX_TAG` itself, so `ps -E` never shows the tag on it. It is
/// found as the direct parent of a matched `nono` that carries the tag or names a path under
/// `layout.root` on its command line. A `nono` whose parent matches neither leaves that parent
/// alone. Other tagged processes are left alone. Exits 0 when no `nono` exists or all are gone;
/// exits nonzero with a one-line reason on stderr when one survives KILL.
#[must_use]
pub fn stop_agent_script(key: &TaskKey, layout: &MacLayout, wait_secs: u32) -> String {
    let (tag, root) = (tag_pattern(key), path_re(&[&layout.root]));
    let find = select(TAG_AWK, &tag, "(^|/)nono$");
    let ticks = wait_secs.saturating_mul(2);
    format!(
        r#"{PRELUDE}{WAIT}snap=$(moor_ps)
nono=$({find})
[ -n "$nono" ] || exit 0
moor_scope
shells=$(printf '%s\n' "$snap" | {SCOPE_ENV} LC_ALL=C KIDS="$nono" TAG={} PAT={} awk {} 2>/dev/null | moor_confirm)
pids="$nono $shells"
for p in $pids; do kill -TERM "$p" 2>/dev/null; done
moor_wait "$pids" {ticks}
for p in $pids; do kill -0 "$p" 2>/dev/null && kill -KILL "$p" 2>/dev/null; done
moor_wait "$pids" 4
moor_alive "$pids" && {{ echo "moor: agent survived KILL: $pids" >&2; exit 1; }}
exit 0
"#,
        quote(&tag),
        quote(&root),
        quote(&format!("{SKIP_AWK}{PARENT_AWK}"))
    )
}

/// TERM each recorded pgid, every tagged pid and every path pid, wait up to 2s for the whole set,
/// then KILL the survivors. Prints `killed <pid>` for each pid that is gone.
///
/// Each signal goes out in a few `kill` calls (200 pids each) and one shared wait polls the set,
/// so the run takes the same few seconds for 3 pids or 300. A pgid of 1 or less is refused
/// before any signal is sent, and the script's own process group is never signalled. Exits 0 on
/// success; exits nonzero with a one-line reason on stderr when a process survives KILL.
///
/// Release kills ANY process whose arguments name a path under the Task root `<root>/`, including
/// an owner's own ssh or monitor shell sitting in that directory (see [`path_pids_cmd`]).
///
/// Every `awk` and `ps` that reads the snapshot runs with `LC_ALL=C` and `2>/dev/null`, so its
/// diagnostics, which can quote environment text, never reach the captured output.
#[must_use]
pub fn kill_tree_script(key: &TaskKey, layout: &MacLayout, pgids: &[i64]) -> String {
    let refusals = pgids
        .iter()
        .find(|g| **g <= 1)
        .map_or_else(String::new, |g| {
            format!("echo 'moor: refusing pgid {g}' >&2; exit 1\n")
        });
    let groups: Vec<String> = pgids
        .iter()
        .filter(|g| **g > 1)
        .map(i64::to_string)
        .collect();
    let mut s = format!(
        "{PRELUDE}{SET_OPS}{refusals}snap=$(moor_ps)\npids=$( {{ {}; {}; }} | sort -u )\nmypg=$(ps -o pgid= -p $$ 2>/dev/null | tr -d ' ')\ngroups=\"{}\"\n",
        select(TAG_AWK, &tag_pattern(key), ""),
        select(PATH_AWK, &path_re(&[&layout.root]), ""),
        groups.join(" ")
    );
    s.push_str(
        r#"sig() { for g in $groups; do [ "$g" = "$mypg" ] || kill -"$1" -- "-$g" 2>/dev/null; done
  moor_sig "$1" $2; }
all=$pids
sig TERM "$all"
moor_settle "$all" 10
sig KILL "$live"
moor_settle "$all" 5
bad=$live
printf '%s\n' $all | BAD="$bad" LC_ALL=C awk 'BEGIN{n=split(ENVIRON["BAD"],b," ");for(i=1;i<=n;i++)x[b[i]]=1}NF&&!($1 in x){print "killed " $1}' 2>/dev/null
[ -z "$bad" ] || { echo "moor: could not kill:$(printf ' %s' $bad)" >&2; exit 1; }
exit 0
"#,
    );
    s
}

/// Kill only an orphaned `ibtoold` (parent pid 1) whose arguments or open files (`lsof -p`)
/// name `layout.sim` or `layout.home`.
///
/// A path matches only as `<dir>/...` or exactly `<dir>`. An `ibtoold` with a live parent, or
/// one that names neither directory, is never signalled. Prints `killed <pid>`; exits
/// nonzero with a one-line reason on stderr when the process survives KILL.
#[must_use]
pub fn orphan_ibtoold_script(layout: &MacLayout) -> String {
    let re = quote(&path_re(&[&layout.sim, &layout.home]));
    format!(
        r#"{PRELUDE}{WAIT}snap=$(moor_ps)
list=$(moor_scope; printf '%s\n' "$snap" | {SCOPE_ENV} LC_ALL=C PAT={re} awk {awk} 2>/dev/null)
pids=""
while read -r p named; do
  [ -n "$p" ] || continue
  if [ "$named" != 1 ] && lsof -p "$p" -Fn 2>/dev/null | sed -n 's/^n//p' | PAT={re} LC_ALL=C awk '$0~ENVIRON["PAT"]{{f=1}}END{{exit !f}}' 2>/dev/null; then named=1; fi
  [ "$named" = 1 ] && pids="$pids $p"
done <<MOOR_EOF
$list
MOOR_EOF
[ -n "$pids" ] || exit 0
for p in $pids; do kill -TERM "$p" 2>/dev/null; done
moor_wait "$pids" 4
for p in $pids; do kill -0 "$p" 2>/dev/null && kill -KILL "$p" 2>/dev/null; done
moor_wait "$pids" 2
moor_alive "$pids" && {{ echo "moor: ibtoold survived KILL:$pids" >&2; exit 1; }}
for p in $pids; do echo "killed $p"; done
exit 0
"#,
        awk = quote(&format!("{SKIP_AWK}{IBTOOLD_AWK}"))
    )
}
