//! Tests for the release scripts: the generated text, and the matchers run under `sh`
//! against canned `ps` output.

use std::fmt::Write as _;
use std::process::{Command, Stdio};

use super::{
    kill_tree_script, orphan_ibtoold_script, path_pids_cmd, snapshot_cmd, stop_agent_script,
    tag_pattern, tagged_pids_cmd,
};
use crate::name::{BoxPath, TaskName};
use crate::task_key::{MacLayout, TaskKey};

mod stderr;

fn key(s: &str) -> TaskKey {
    TaskKey::new(&TaskName::parse(s).unwrap())
}

fn layout(s: &str) -> MacLayout {
    MacLayout::new(&BoxPath::restore("/w").unwrap(), &key(s)).unwrap()
}

/// Runs `snippet` under `sh` with `moor_ps` printing `ps`; returns the pids printed.
///
/// Columns are pid, ppid, pgid, session, then the command. In `ps`, `@ME@` becomes the script's
/// pid, `@SELF@` its session id and `@TOPPG@` the process group of its topmost non-init ancestor.
/// Canned pids are not alive, so `moor_confirm` passes every pid through. A snippet that does not
/// take its own snapshot gets one first.
fn pids_from(ps: &str, snippet: &str) -> Vec<String> {
    let snap = if snippet.contains("snap=$(") {
        String::new()
    } else {
        snapshot_cmd()
    };
    let script = format!(
        "moor_confirm() {{ cat; }}\nmoor_ps() {{ sed -e \"s/@ME@/$$/g\" -e \"s/@SELF@/$(ps -o sess= -p $$ | tr -d ' ')/g\" \\\n\
         -e \"s/@TOPPG@/$(ps -o pgid= -p \"$(moor_ancestors | tail -n 1)\" | tr -d ' ')/g\" <<'MOOR_EOF'\n{ps}\nMOOR_EOF\n}}\n{snap}{snippet}"
    );
    let out = Command::new("sh").arg("-c").arg(script).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

const NONCE: &str = "0123456789abcdef";

fn tagged_line(pid: u32, tag: &str) -> String {
    format!("{pid} 1 {pid} s9 /usr/bin/node app.js HOME=/h MOOR_BOX_TAG={tag} PATH=/bin")
}

/// The pattern anchors both ends and escapes the key.
#[test]
fn tag_pattern_is_anchored() {
    assert_eq!(tag_pattern(&key("app")), "^app-[0-9a-f]{16}$");
}

/// Tag `app` matches its own nonce only.
#[test]
fn tag_match_own_nonce() {
    let ps = [
        tagged_line(90001, &format!("app-{NONCE}")),
        tagged_line(90002, &format!("other-{NONCE}")),
    ]
    .join("\n");
    assert_eq!(pids_from(&ps, &tagged_pids_cmd(&key("app"))), ["90001"]);
}

/// Task `app` never matches a process tagged `app-two-<nonce>`.
#[test]
fn tag_match_rejects_longer_key() {
    let ps = tagged_line(90001, &format!("app-two-{NONCE}"));
    assert!(pids_from(&ps, &tagged_pids_cmd(&key("app"))).is_empty());
    assert_eq!(pids_from(&ps, &tagged_pids_cmd(&key("app-two"))), ["90001"]);
}

/// A short nonce, a long nonce, uppercase hex and trailing text do not match.
#[test]
fn tag_match_rejects_bad_nonce() {
    let ps = [
        tagged_line(90001, "app-0123456789abcde"),
        tagged_line(90002, "app-0123456789abcdef0"),
        tagged_line(90003, "app-0123456789ABCDEF"),
        tagged_line(90004, "app-0123456789abcdef-x"),
        tagged_line(90005, "xapp-0123456789abcdef"),
    ]
    .join("\n");
    assert!(pids_from(&ps, &tagged_pids_cmd(&key("app"))).is_empty());
}

/// The tag must be its own variable, not a suffix of another name.
#[test]
fn tag_match_rejects_other_variable() {
    let ps =
        format!("90001 1 90001 s9 /bin/x NOT_MOOR_BOX_TAG=app-{NONCE} MOOR_BOX_TAGX=app-{NONCE}");
    assert!(pids_from(&ps, &tagged_pids_cmd(&key("app"))).is_empty());
}

/// Path `/w/app` matches itself and children, never `/w/app-two` or `/x/w/app`.
#[test]
fn path_match_needs_boundary() {
    let ps = [
        "90001 1 90001 s9 /bin/cat /w/app",
        "90002 1 90002 s9 /bin/cat /w/app/work/f",
        "90003 1 90003 s9 /bin/cat /w/app-two/work/f",
        "90004 1 90004 s9 /bin/cat /x/w/app/f",
        "90005 1 90005 s9 /bin/cat /w/apple",
        "90006 1 90006 s9 sh -c cd '/w/app/work' && run",
        "90007 1 90007 s9 /bin/cat /w/app.bak/f",
        "90008 1 90008 s9 /bin/cat --dir=/w/app/f",
    ]
    .join("\n");
    assert_eq!(
        pids_from(&ps, &path_pids_cmd(&layout("app"))),
        ["90001", "90002", "90006", "90008"]
    );
}

/// The script's ancestors (here the test process) are never printed.
#[test]
fn ancestor_excluded() {
    let me = std::process::id();
    let ps = format!("{me} 1 {me} s9 sh -c /w/app\n90003 1 90003 s9 other /w/app");
    assert_eq!(pids_from(&ps, &path_pids_cmd(&layout("app"))), ["90003"]);
}

/// A canned child of the script (ppid == `$$`) is skipped.
#[test]
fn child_of_script_excluded() {
    let snippet = format!(
        "moor_confirm() {{ cat; }}\nmoor_ps() {{ printf '%s\\n' \"90001 $$ 90001 s9 ps /w/app\" '90002 1 90002 s9 x /w/app'; }}\n{}{}",
        snapshot_cmd(),
        path_pids_cmd(&layout("app"))
    );
    let out = Command::new("sh").arg("-c").arg(snippet).output().unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "90002");
}

/// Both matchers exit 0 with no output when nothing matches.
#[test]
fn matchers_empty_ok() {
    assert!(pids_from("", &tagged_pids_cmd(&key("app"))).is_empty());
    assert!(pids_from("", &path_pids_cmd(&layout("app"))).is_empty());
}

/// Regex metacharacters in a key are escaped.
#[test]
fn key_is_escaped() {
    assert!(super::ere_escape("a.b+c").contains("a\\.b\\+c"));
}

/// The stop script signals only a tagged `nono` and waits then KILLs it.
#[test]
fn stop_agent_text() {
    let s = stop_agent_script(&key("app"), &layout("app"), 5);
    assert!(s.contains("^app-[0-9a-f]{16}$"), "{s}");
    assert!(s.contains("nono$"));
    let (term, kill) = (s.find("kill -TERM").unwrap(), s.find("kill -KILL").unwrap());
    assert!(term < kill);
    assert!(
        s.contains("moor_wait \"$pids\" 10"),
        "5 seconds is 10 half-second ticks"
    );
    assert!(s.contains("exit 1"));
}

/// Runs the stop script's selection (everything before the first signal); returns the pids.
fn stop_selects(task: &str, ps: &str) -> Vec<String> {
    let s = stop_agent_script(&key(task), &layout(task), 1);
    let head = s.split_once("for p in $pids; do kill -TERM").unwrap().0;
    let mut got = pids_from(ps, &format!("{head}printf '%s\\n' $pids"));
    got.sort();
    got
}

fn nono(pid: u32, parent: u32, tag: &str) -> String {
    format!("{pid} {parent} {pid} s9 /opt/bin/nono run MOOR_BOX_TAG={tag}")
}

/// A launch shell: no tag in its environment, the Task's paths in its script text.
fn launch_shell(pid: u32, task: &str) -> String {
    format!("{pid} 1 {pid} s9 sh -c cd /w/{task}/work; rm -rf /w/{task}/launch/.moor-cred-x /w/{task}/tmp/y")
}

/// The stop script picks the nono of this Task, not other tagged processes.
#[test]
fn stop_agent_selects_nono_only() {
    let ps = [
        nono(90001, 1, &format!("app-{NONCE}")),
        format!("90002 1 90002 s9 /usr/bin/node MOOR_BOX_TAG=app-{NONCE}"),
        nono(90003, 1, &format!("app-two-{NONCE}")),
        format!("90004 1 90004 s9 /opt/bin/nonox run MOOR_BOX_TAG=app-{NONCE}"),
    ]
    .join("\n");
    assert_eq!(stop_selects("app", &ps), ["90001"]);
}

/// The untagged launch shell that is the parent of the tagged nono is selected with it.
#[test]
fn stop_agent_selects_launch_shell() {
    let ps = [
        launch_shell(90010, "app"),
        nono(90011, 90010, &format!("app-{NONCE}")),
    ]
    .join("\n");
    assert_eq!(stop_selects("app", &ps), ["90010", "90011"]);
}

/// The same layout for Task `app-two` is neither a shell nor a nono of Task `app`.
#[test]
fn stop_agent_ignores_longer_key_shell() {
    let ps = [
        launch_shell(90010, "app-two"),
        nono(90011, 90010, &format!("app-two-{NONCE}")),
        nono(90012, 90020, &format!("app-{NONCE}")),
        launch_shell(90020, "app-two"),
    ]
    .join("\n");
    assert_eq!(stop_selects("app", &ps), ["90012"]);
}

/// A shell that carries the tag at exec is a match without a Task path.
#[test]
fn stop_agent_selects_tagged_parent() {
    let ps = [
        format!("90010 1 90010 s9 sh -c run MOOR_BOX_TAG=app-{NONCE}"),
        nono(90011, 90010, &format!("app-{NONCE}")),
    ]
    .join("\n");
    assert_eq!(stop_selects("app", &ps), ["90010", "90011"]);
}

/// The parent of a nono is taken only when it matches the tag or the root path; an unrelated
/// shell, and any sibling or grandparent, stays alone.
#[test]
fn stop_agent_spares_unrelated_parent() {
    let ps = [
        "90030 1 90030 s9 -zsh HOME=/Users/box".to_owned(),
        nono(90031, 90030, &format!("app-{NONCE}")),
        launch_shell(90032, "app"),
        "90033 90032 90033 s9 sleep 100".to_owned(),
    ]
    .join("\n");
    assert_eq!(stop_selects("app", &ps), ["90031"]);
}

/// The kill script refuses pgid 0 and 1 before sending a signal.
#[test]
fn kill_tree_refuses_low_pgid() {
    for bad in [-5, 0, 1] {
        let s = kill_tree_script(&key("app"), &layout("app"), &[bad]);
        let refuse = s.find(&format!("refusing pgid {bad}")).unwrap();
        assert!(refuse < s.find("sig TERM").unwrap());
        let out = Command::new("sh").arg("-c").arg(&s).output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&out.stderr).contains("refusing pgid"));
    }
}

/// The kill script lists recorded pgids, both matchers, TERM before KILL, and the output line.
#[test]
fn kill_tree_text() {
    let s = kill_tree_script(&key("app"), &layout("app"), &[4242, 77]);
    assert!(s.contains("groups=\"4242 77\""), "{s}");
    assert!(s.contains("^app-[0-9a-f]{16}$"));
    assert!(s.contains("/w/app"));
    assert!(s.contains("print \"killed \" $1"));
    assert!(s.find("sig TERM").unwrap() < s.find("sig KILL").unwrap());
}

/// With no matching process and no pgid the kill script prints nothing and exits 0.
#[test]
fn kill_tree_nothing_to_kill() {
    let s = kill_tree_script(&key("app"), &layout("app"), &[]);
    let script = format!("moor_ps() {{ :; }}\n{s}");
    let out = Command::new("sh").arg("-c").arg(script).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.is_empty());
}

/// The ibtoold script names only orphans (ppid 1) and matches sim/home by boundary.
#[test]
fn orphan_ibtoold_selection() {
    let l = layout("app");
    let s = orphan_ibtoold_script(&l);
    assert!(s.contains("/w/app/sim") && s.contains("/w/app/home"), "{s}");
    assert!(s.contains("lsof -p"));
    assert!(s.contains("ibtoold"));
    let ps = [
        "90001 1 90001 s9 /usr/bin/ibtoold --dir /w/app/sim/x",
        "90002 5 90002 s9 /usr/bin/ibtoold --dir /w/app/sim/x",
        "90003 1 90003 s9 /usr/bin/ibtoold --dir /w/app-two/sim/x",
        "90004 1 90004 s9 /usr/bin/other --dir /w/app/sim/x",
        "90005 1 90005 s9 /usr/bin/ibtoold --dir /w/app/home",
    ]
    .join("\n");
    // Run only the selection: the pid and named-flag list, before any lsof or kill.
    let head = s.split_once("pids=\"\"\nwhile read").unwrap().0;
    let out = pids_from(&ps, &format!("{head}printf '%s\\n' \"$list\""));
    assert_eq!(out, ["90001", "1", "90003", "0", "90005", "1"]);
}

/// A grandchild subshell of the script whose argv names the root is never selected, at any depth.
#[test]
fn descendant_at_any_depth_excluded() {
    let ps = [
        "90001 @ME@ 90001 s9 sh -c cd /w/app",
        "90002 90001 90002 s9 sh -c cd /w/app",
        "90003 90002 90003 s9 sh -c cd /w/app MOOR_BOX_TAG=app-0123456789abcdef",
        "90004 1 90004 s9 sh -c cd /w/app",
    ]
    .join("\n");
    assert_eq!(pids_from(&ps, &path_pids_cmd(&layout("app"))), ["90004"]);
    assert!(pids_from(&ps, &tagged_pids_cmd(&key("app"))).is_empty());
}

/// A sibling in the script's own session that names the root is never selected.
#[test]
fn same_session_sibling_excluded() {
    let tag = format!("MOOR_BOX_TAG=app-{NONCE}");
    let ps = [
        "90001 1 90001 @SELF@ ssh-session /w/app".to_owned(),
        format!("90002 1 90002 @SELF@ node {tag}"),
        "90003 1 90003 s9 cat /w/app/x".to_owned(),
    ]
    .join("\n");
    assert_eq!(pids_from(&ps, &path_pids_cmd(&layout("app"))), ["90003"]);
    assert!(pids_from(&ps, &tagged_pids_cmd(&key("app"))).is_empty());
}

/// A Task process in another session is selected by root path or by tag, and `app-two` is not
/// selected for `app` even beside the session being skipped.
#[test]
fn other_session_selected() {
    let tag = format!("MOOR_BOX_TAG=app-{NONCE}");
    let ps = [
        "90001 1 90001 4242 /usr/bin/node /w/app/work/x".to_owned(),
        format!("90002 1 90002 4242 /usr/bin/node {tag}"),
        "90003 1 90003 4242 /usr/bin/node /w/app-two/work/x".to_owned(),
        "90004 1 90004 @SELF@ sh -c /w/app".to_owned(),
    ]
    .join("\n");
    assert_eq!(pids_from(&ps, &path_pids_cmd(&layout("app"))), ["90001"]);
    assert_eq!(pids_from(&ps, &tagged_pids_cmd(&key("app"))), ["90002"]);
}

/// `sess` printing `val` for this shell stands in for a `ps` that cannot report the session.
fn path_with_sess(val: &str, ps: &str) -> Vec<String> {
    let snippet = format!(
        "ps() {{ case \"$*\" in (*sess*) echo '{val}';; (*) command ps \"$@\";; esac; }}\n{}",
        path_pids_cmd(&layout("app"))
    );
    pids_from(ps, &snippet)
}

/// A session id of 0 or none is not used: a Task process in another group is still selected,
/// and descendants of the script are still skipped.
#[test]
fn unusable_session_excludes_only_descendants() {
    let ps = [
        "90001 1 90001 0 cat /w/app/x",
        "90002 @ME@ 90002 0 cat /w/app/y",
        "90003 90002 90002 0 cat /w/app/z",
    ]
    .join("\n");
    for val in ["0", ""] {
        assert_eq!(path_with_sess(val, &ps), ["90001"], "sess {val:?}");
    }
    let tag = format!("MOOR_BOX_TAG=app-{NONCE}");
    let tagged = format!("90001 1 90001 0 node {tag}\n90002 @ME@ 90002 0 node {tag}");
    let snippet = format!(
        "ps() {{ case \"$*\" in (*sess*) echo 0;; (*) command ps \"$@\";; esac; }}\n{}",
        tagged_pids_cmd(&key("app"))
    );
    assert_eq!(pids_from(&tagged, &snippet), ["90001"]);
}

/// A usable session id that every process shares would skip everything, so it is not used.
#[test]
fn session_shared_by_all_is_not_used() {
    let ps = "90001 1 90001 @SELF@ cat /w/app/x";
    assert_eq!(pids_from(ps, &path_pids_cmd(&layout("app"))), ["90001"]);
}

/// The stop script skips a nono in the script's session and still takes one in another session.
#[test]
fn stop_agent_respects_session() {
    let ps = [
        nono(90001, 1, &format!("app-{NONCE}")).replace(" s9 ", " @SELF@ "),
        nono(90002, 1, &format!("app-{NONCE}")).replace(" s9 ", " 4242 "),
    ]
    .join("\n");
    assert_eq!(stop_selects("app", &ps), ["90002"]);
}

/// Column lists for a `moor_ps` backed by the real `ps`, with and without the session column.
const REAL_COLS: [&str; 2] = ["pid=,ppid=,pgid=,sess=,args=", "pid=,ppid=,pgid=,args="];

/// A Task whose root exists nowhere, so no process of it can exist.
fn nowhere() -> (TaskKey, MacLayout) {
    let root = format!("/tmp/moor-nowhere-{}", std::process::id());
    let k = key("app");
    (
        k.clone(),
        MacLayout::new(&BoxPath::restore(&root).unwrap(), &k).unwrap(),
    )
}

/// Runs `script` under `sh -c` with the real `ps`; returns (exit code, stdout).
fn run_real(cols: &str, script: &str) -> (Option<i32>, String) {
    let out = Command::new("sh")
        .arg("-c")
        .arg(format!("moor_ps() {{ ps -e -ww -o {cols}; }}\n{script}"))
        .output()
        .unwrap();
    (out.status.code(), String::from_utf8(out.stdout).unwrap())
}

/// With nothing of the Task present the kill script's own subshells are not killed or reported.
#[test]
fn real_kill_tree_nothing_present() {
    let (k, l) = nowhere();
    for cols in REAL_COLS {
        let (code, out) = run_real(cols, &kill_tree_script(&k, &l, &[]));
        assert_eq!((code, out.as_str()), (Some(0), ""), "{cols}");
    }
}

/// The matchers print no pid when nothing of the Task exists, even when run in subshells that
/// carry the root in their argv, the way the verify script runs them.
#[test]
fn real_matchers_nothing_present() {
    let (k, l) = nowhere();
    let root = l.root.as_str();
    let script = format!(
        "root='{root}'\n{}\npids() {{ ( : \n{}\n ) 2>/dev/null; ( :\n{}\n ) 2>/dev/null; }}\nfor p in $(pids | sort -u); do echo \"left: process $p\"; done\n",
        snapshot_cmd(),
        tagged_pids_cmd(&k),
        path_pids_cmd(&l)
    );
    for cols in REAL_COLS {
        assert_eq!(run_real(cols, &script), (Some(0), String::new()), "{cols}");
    }
}

/// Runs the real kill script under `sh` with 300 canned Task processes, a recording `kill` and
/// `sleep`, and a `ps` that reports them alive until a KILL; returns (stdout, call log).
fn kill_300(stubborn: bool) -> (String, String) {
    let mut rows = String::new();
    for p in 900_000..900_300 {
        write!(rows, "{p} 1 {p} 0 cat /w/app/x\\n").unwrap();
    }
    let log = format!("/tmp/moor-kill300-{}-{stubborn}", std::process::id());
    let script = format!(
        "moor_confirm() {{ cat; }}\nmoor_ps() {{ printf '{rows}'; }}\n\
         kill() {{ echo \"kill $*\" >> {log}; case \"$1\" in (-KILL) dead=1;; esac; }}\n\
         sleep() {{ echo \"sleep $*\" >> {log}; }}\n\
         ps() {{ case \"$*\" in (*pid=*-p*,*) {stub};; (*) command ps \"$@\";; esac; }}\n{}",
        kill_tree_script(&key("app"), &layout("app"), &[]),
        stub = if stubborn {
            "[ -n \"$dead\" ] || echo \"$*\" | tr ' ,' '\\n\\n' | grep '^9'"
        } else {
            ":"
        }
    );
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ticks = 0;
    while child.try_wait().unwrap().is_none() {
        assert!(ticks < 50, "kill script still running after 5s");
        std::thread::sleep(std::time::Duration::from_millis(100));
        ticks += 1;
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let calls = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    (String::from_utf8(out.stdout).unwrap(), calls)
}

/// 300 pids take a handful of `kill` calls and one shared wait, never a call or sleep per pid.
#[test]
fn kill_tree_cost_is_flat_in_pids() {
    for stubborn in [false, true] {
        let (out, calls) = kill_300(stubborn);
        let kills = calls.lines().filter(|l| l.starts_with("kill")).count();
        let sleeps = calls.lines().filter(|l| l.starts_with("sleep")).count();
        assert!(kills <= 6, "{kills} kill calls\n{calls}");
        assert!(sleeps <= 15, "{sleeps} sleeps");
        assert_eq!(out.lines().count(), 300, "every pid reported killed");
        assert!(out.lines().all(|l| l.starts_with("killed 9")));
        assert!(calls.contains("kill -TERM 900000 "), "{calls}");
        assert_eq!(stubborn, calls.contains("kill -KILL"), "{calls}");
    }
}

/// Every `case` pattern in every generated script opens with `(`, which macOS `sh` needs
/// inside `$( ... )`.
#[test]
fn case_patterns_open_with_paren() {
    let (k, l) = (key("app"), layout("app"));
    let scripts = [
        kill_tree_script(&k, &l, &[4242]),
        stop_agent_script(&k, &l, 5),
        orphan_ibtoold_script(&l),
        snapshot_cmd(),
        tagged_pids_cmd(&k),
        path_pids_cmd(&l),
    ];
    for s in &scripts {
        let mut in_case = false;
        for line in s.lines().map(str::trim_start) {
            if line.starts_with("case ") && line.contains(" in") {
                in_case = true;
                assert!(
                    line.rsplit(" in").next().is_some_and(|rest| {
                        let rest = rest.trim();
                        rest.is_empty() || rest.starts_with('(')
                    }),
                    "{line}"
                );
            } else if in_case && line.contains(')') && !line.starts_with("esac") {
                assert!(line.starts_with('('), "case pattern without paren: {line}");
            }
            if line.contains("esac") {
                in_case = false;
            }
        }
    }
}

/// A realistic macOS snapshot where `ps -o sess=` is 0 on every row: Task processes in other
/// groups are all selected, the script's own descendants are not, and nothing else is dropped.
#[test]
fn macos_all_zero_sessions() {
    let tag = format!("MOOR_BOX_TAG=app-{NONCE}");
    let ps = [
        "1 0 1 0 /sbin/launchd".to_owned(),
        "400 1 400 0 /usr/sbin/sshd".to_owned(),
        "5000 400 5000 0 sshd: me".to_owned(),
        "5001 5000 5001 0 -zsh".to_owned(),
        "6001 1 6001 0 /usr/bin/node /w/app/work/a.js".to_owned(),
        format!("6002 1 6002 0 /usr/bin/node b.js {tag}"),
        "6003 6002 6002 0 /bin/sh /w/app/work/run.sh".to_owned(),
        "6004 1 6004 0 /usr/bin/node /w/app-two/work/c.js".to_owned(),
        "7001 @ME@ 7001 0 awk /w/app".to_owned(),
        "7002 7001 7001 0 cat /w/app/x".to_owned(),
    ]
    .join("\n");
    assert_eq!(
        path_with_sess("0", &ps),
        ["6001", "6003"],
        "path matches by root"
    );
    let tagged = format!("{ps}\n7003 @ME@ 7003 0 node {tag}");
    let snippet = format!(
        "ps() {{ case \"$*\" in (*sess*) echo 0;; (*) command ps \"$@\";; esac; }}\n{}",
        tagged_pids_cmd(&key("app"))
    );
    assert_eq!(pids_from(&tagged, &snippet), ["6002"]);
}
