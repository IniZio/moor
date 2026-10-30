#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value,
    clippy::disallowed_methods,
    missing_docs,
    unreachable_pub,
    reason = "tests assert on fixed fixtures"
)]

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::json;

use rig::{Rig, HEADERS};

mod broker;
mod claim;
mod gc;
mod holds;
mod hooks;
mod linked;
mod missing;
mod pull;
mod relay;
mod rig;
mod sleep;

fn ino(path: &Path) -> u64 {
    std::fs::metadata(path).unwrap().ino()
}

#[test]
fn a_request_missing_a_header_is_refused_naming_it() {
    let rig = Rig::start();
    for (missing, _) in HEADERS {
        let headers: Vec<_> = HEADERS.into_iter().filter(|(k, _)| *k != missing).collect();
        let err = rig
            .call_with("exec", json!({"command": "true"}), &headers)
            .unwrap_err();
        assert!(err.ends_with(&format!("missing header {missing}")), "{err}");
    }
}

#[test]
fn sync_copies_the_tree_and_a_second_sync_sends_only_the_change() {
    let rig = Rig::start();
    rig.write("a.txt", "one");
    rig.write("sub/b.txt", "two");
    rig.write(".git/HEAD", "ref");
    rig.write("node_modules/m/index.js", "native");
    rig.call("sync", json!({})).unwrap();
    let copy = rig.copy();
    assert_eq!(std::fs::read_to_string(copy.join("a.txt")).unwrap(), "one");
    assert_eq!(
        std::fs::read_to_string(copy.join("sub/b.txt")).unwrap(),
        "two"
    );
    assert!(
        copy.join("node_modules/m/index.js").exists(),
        "node_modules is kept"
    );
    assert!(!copy.join(".git").exists(), ".git is left out");
    let (a, b) = (ino(&copy.join("a.txt")), ino(&copy.join("sub/b.txt")));
    rig.write("a.txt", "one, changed");
    rig.call("sync", json!({})).unwrap();
    let changed = std::fs::read_to_string(copy.join("a.txt")).unwrap();
    assert_eq!(changed, "one, changed");
    assert_ne!(ino(&copy.join("a.txt")), a, "the changed file was sent");
    assert_eq!(
        ino(&copy.join("sub/b.txt")),
        b,
        "the unchanged file was not sent again"
    );
}

#[test]
fn sync_removes_a_file_deleted_in_the_box() {
    let rig = Rig::start();
    rig.write("keep.txt", "k");
    rig.write("gone.txt", "g");
    rig.call("sync", json!({})).unwrap();
    assert!(rig.copy().join("gone.txt").exists());
    std::fs::remove_file(rig.tree.join("gone.txt")).unwrap();
    rig.call("sync", json!({})).unwrap();
    assert!(
        !rig.copy().join("gone.txt").exists(),
        "deleted on the machine too"
    );
    assert!(rig.copy().join("keep.txt").exists());
}

#[test]
fn exec_runs_in_the_synced_copy_and_returns_code_and_output() {
    let rig = Rig::start();
    rig.write("a.txt", "hello");
    rig.call("sync", json!({})).unwrap();
    let command = "pwd; cat a.txt; echo oops >&2; exit 3";
    let out = rig.call("exec", json!({ "command": command })).unwrap();
    assert_eq!(out["exit_code"], 3, "{out}");
    assert_eq!(out["timed_out"], false);
    let output = out["output"].as_str().unwrap();
    assert!(
        output.contains(&rig.copy().display().to_string()),
        "{output}"
    );
    assert!(
        output.contains("hello") && output.contains("oops"),
        "{output}"
    );
    assert_eq!(out["truncated"], false);
}

#[test]
fn exec_output_over_64_kib_is_cut_to_its_tail() {
    let rig = Rig::start();
    rig.call("sync", json!({})).unwrap();
    let command = "head -c 70000 /dev/zero | tr '\\0' a; printf END";
    let out = rig.call("exec", json!({ "command": command })).unwrap();
    let output = out["output"].as_str().unwrap();
    assert_eq!(output.len(), 64 * 1024);
    assert!(output.ends_with("aEND"), "the tail is kept");
    assert_eq!(out["truncated"], true);
    assert_eq!(out["exit_code"], 0);
}

#[test]
fn exec_past_its_timeout_is_killed_and_says_so() {
    let rig = Rig::start();
    rig.call("sync", json!({})).unwrap();
    let err = rig
        .call("exec", json!({"command": "true", "timeout_secs": 3601}))
        .unwrap_err();
    assert!(err.contains("3600"), "{err}");
    let started = Instant::now();
    let command = "echo started; sleep 30";
    let out = rig
        .call("exec", json!({"command": command, "timeout_secs": 1}))
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "returned at the timeout"
    );
    assert_eq!(out["timed_out"], true, "{out}");
    assert_eq!(out["exit_code"], json!(null));
    assert!(out["output"].as_str().unwrap().contains("started"), "{out}");
}

#[test]
fn a_bad_slot_header_is_refused() {
    let rig = Rig::start();
    for bad in ["-1", "6554", "two"] {
        let headers: Vec<_> = HEADERS
            .into_iter()
            .map(|(k, v)| (k, if k == "X-Moor-Machine-Slot" { bad } else { v }))
            .collect();
        let err = rig
            .call_with("machine_info", json!({}), &headers)
            .unwrap_err();
        assert!(
            err.contains("X-Moor-Machine-Slot") && err.contains(bad),
            "{err}"
        );
    }
}

#[test]
fn machine_info_gives_the_slot_and_its_port_offset() {
    let rig = Rig::start();
    let headers: Vec<_> = HEADERS
        .into_iter()
        .map(|(k, v)| (k, if k == "X-Moor-Machine-Slot" { "2" } else { v }))
        .collect();
    let info = rig.call_with("machine_info", json!({}), &headers).unwrap();
    assert_eq!(
        info,
        json!({"box": 7, "machine": "minion", "slot": 2, "port_offset": 20, "facts": {}, "sync_exclude": []})
    );
}

/// Waits up to 10 s for the pid the command wrote to `file` in the Box's copy.
fn written_pid(rig: &Rig, file: &str) -> u32 {
    let path = rig.copy().join(file);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(pid) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            return pid;
        }
        assert!(Instant::now() < deadline, "the command never wrote {file}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Whether `pid` still runs: it exists and is not a zombie.
fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit(')')
            .next()
            .is_some_and(|s| !s.trim_start().starts_with('Z'))
    })
}

fn gone_within_6s(pid: u32) {
    let started = Instant::now();
    while alive(pid) {
        assert!(
            started.elapsed() < Duration::from_secs(6),
            "the command on the machine outlived its call"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn exec_past_its_timeout_ends_the_command_on_the_machine() {
    let rig = Rig::start();
    rig.call("sync", json!({})).unwrap();
    let command = "echo $$ > pid; exec sleep 300";
    let out = rig
        .call("exec", json!({"command": command, "timeout_secs": 1}))
        .unwrap();
    assert_eq!(out["timed_out"], true, "{out}");
    gone_within_6s(written_pid(&rig, "pid"));
}

#[test]
fn a_call_hung_up_on_ends_the_command_on_the_machine() {
    let rig = Rig::start();
    rig.call("sync", json!({})).unwrap();
    let call = rig.send_call("exec", json!({"command": "echo $$ > pid; exec sleep 300"}));
    let pid = written_pid(&rig, "pid");
    std::thread::sleep(Duration::from_secs(1));
    assert!(alive(pid), "the command runs while its call is open");
    drop(call);
    gone_within_6s(pid);
}

#[test]
fn exec_runs_quotes_substitutions_and_heredocs_as_typed() {
    let rig = Rig::start();
    rig.call("sync", json!({})).unwrap();
    let command = "printf '%s|' \"it's\" \"$(echo sub)\" 'a;b'; echo\n\
                   cat <<'MOOR_END'\nMOOR_END_\nMOOR_END\necho end; exit 4";
    let out = rig.call("exec", json!({ "command": command })).unwrap();
    assert_eq!(out["output"], "it's|sub|a;b|\nMOOR_END_\nend\n", "{out}");
    assert_eq!(out["exit_code"], 4, "{out}");
}

#[test]
fn an_ssh_header_that_is_not_a_strict_target_is_refused() {
    let rig = Rig::start();
    let with_ssh = |ssh: &'static str| -> Vec<(&str, &str)> {
        HEADERS
            .into_iter()
            .map(|(k, v)| (k, if k == "X-Moor-Machine-Ssh" { ssh } else { v }))
            .collect()
    };
    for bad in ["user@-oProxyCommand=x", "-x@h"] {
        let err = rig
            .call_with("machine_info", json!({}), &with_ssh(bad))
            .unwrap_err();
        assert!(err.contains("X-Moor-Machine-Ssh"), "{bad}: {err}");
    }
    for good in ["moor@minion", "moor@[::1]:22"] {
        let info = rig.call_with("machine_info", json!({}), &with_ssh(good));
        assert!(info.is_ok(), "{good}: {info:?}");
    }
}

#[test]
fn a_long_call_answers_at_once_and_keeps_the_stream_alive() {
    use std::io::Read;
    let rig = Rig::with_args(&["--keep-alive-secs", "1"]);
    rig.call("sync", json!({})).unwrap();
    let mut call = rig.send_call("exec", json!({"command": "sleep 4; echo done"}));
    call.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let started = Instant::now();
    let mut seen = Vec::new();
    let mut buf = [0u8; 4096];
    let mut head_at = None;
    while started.elapsed() < Duration::from_secs(10) {
        match call.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => seen.extend_from_slice(&buf[..n]),
            Err(_) => {}
        }
        let text = String::from_utf8_lossy(&seen);
        if head_at.is_none() && text.contains("\r\n\r\n") {
            head_at = Some(started.elapsed());
        }
        if text.contains("done") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&seen);
    let head_at = head_at.unwrap_or_else(|| panic!("no response head: {text}"));
    assert!(
        head_at < Duration::from_secs(2),
        "head after {head_at:?}: {text}"
    );
    assert!(text.contains("text/event-stream"), "{text}");
    let body = text.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    let before_result = body.split("\"result\"").next().unwrap_or_default();
    assert!(
        before_result.matches("\n:").count() + usize::from(before_result.starts_with(':')) >= 2
            || before_result.contains("ping"),
        "no keep-alive before the result: {body}"
    );
    assert!(text.contains("done"), "{text}");
}

#[test]
fn a_long_exec_reports_progress_while_it_runs() {
    let rig = Rig::with_args(&["--keep-alive-secs", "1"]);
    rig.call("sync", json!({})).unwrap();
    let (out, progress) =
        rig.call_progress("exec", json!({"command": "sleep 3; echo done"}), &HEADERS);
    assert_eq!(out.unwrap()["output"], "done\n");
    let beats = progress
        .iter()
        .filter(|m| m.starts_with("exec: running for"))
        .count();
    assert!(beats >= 2, "{progress:?}");
}

fn sync_excluding(rig: &Rig, exclude: &[&str]) {
    rig.call("sync", json!({ "exclude": exclude })).unwrap();
}

#[test]
fn sync_leaves_an_excluded_directory_out() {
    let rig = Rig::start();
    rig.write("app/node_modules/m/index.js", "linux");
    rig.write("app/src.js", "src");
    sync_excluding(&rig, &["app/node_modules"]);
    assert!(rig.copy().join("app/src.js").exists());
    assert!(!rig.copy().join("app/node_modules").exists());
}

#[test]
fn a_second_sync_keeps_what_the_machine_built_in_an_excluded_path() {
    let rig = Rig::start();
    rig.write("app/node_modules/m/index.js", "linux");
    rig.write("gone.txt", "g");
    rig.write("keep.txt", "k");
    sync_excluding(&rig, &["app/node_modules"]);
    let own = rig.copy().join("app/node_modules/darwin.node");
    std::fs::create_dir_all(own.parent().unwrap()).unwrap();
    std::fs::write(&own, "machine").unwrap();
    std::fs::remove_file(rig.tree.join("gone.txt")).unwrap();
    sync_excluding(&rig, &["app/node_modules"]);
    assert_eq!(std::fs::read_to_string(&own).unwrap(), "machine");
    assert!(!rig.copy().join("gone.txt").exists());
    assert!(rig.copy().join("keep.txt").exists());
}

#[test]
fn sync_refuses_a_bad_exclude_pattern_naming_it() {
    let rig = Rig::start();
    rig.write("a.txt", "a");
    let too_many: Vec<String> = (0..65).map(|i| format!("p{i}")).collect();
    for bad in ["", "-rf", "--delete-excluded", "a\nb", "a\0b"] {
        let err = rig.call("sync", json!({ "exclude": [bad] })).unwrap_err();
        assert!(err.contains("exclude"), "{err}");
        assert!(err.contains(&format!("{bad:?}")), "{err}");
    }
    let err = rig
        .call("sync", json!({ "exclude": too_many }))
        .unwrap_err();
    assert!(err.contains("at most 64"), "{err}");
    assert!(!rig.copy().join("a.txt").exists(), "nothing was synced");
}

#[test]
fn an_exclude_with_spaces_or_a_star_is_one_rsync_pattern() {
    let rig = Rig::start();
    rig.write("my dir/f.txt", "spaced");
    rig.write("build/a.o", "o");
    rig.write("build/b.o", "o");
    rig.write("build/c.txt", "c");
    sync_excluding(&rig, &["my dir", "build/*.o"]);
    assert!(!rig.copy().join("my dir").exists());
    assert!(!rig.copy().join("build/a.o").exists());
    assert!(!rig.copy().join("build/b.o").exists());
    assert!(rig.copy().join("build/c.txt").exists());
}

/// [`HEADERS`] with the Proxy's exclude header set to `json`.
fn excluding(json: &'static str) -> Vec<(&'static str, &'static str)> {
    let mut headers = HEADERS.to_vec();
    headers.push(("X-Moor-Machine-Exclude", json));
    headers
}

#[test]
fn sync_leaves_alone_the_patterns_the_proxy_sends_and_the_calls_own() {
    let rig = Rig::start();
    let headers = excluding(r#"["node_modules"]"#);
    rig.write("app/node_modules/m/index.js", "linux");
    rig.write("app/build/out.o", "linux");
    rig.write("app/src.js", "src");
    let args = json!({ "exclude": ["app/build"] });
    rig.call_with("sync", args.clone(), &headers).unwrap();
    assert!(rig.copy().join("app/src.js").exists());
    assert!(!rig.copy().join("app/node_modules").exists());
    assert!(!rig.copy().join("app/build").exists());
    let built = ["app/node_modules/darwin.node", "app/build/darwin.o"];
    for rel in built {
        let own = rig.copy().join(rel);
        std::fs::create_dir_all(own.parent().unwrap()).unwrap();
        std::fs::write(&own, "machine").unwrap();
    }
    rig.call_with("sync", args, &headers).unwrap();
    for rel in built {
        let kept = std::fs::read_to_string(rig.copy().join(rel)).unwrap();
        assert_eq!(kept, "machine", "{rel}");
    }
}

#[test]
fn the_box_mirror_request_carries_the_merged_exclude_list() {
    let rig = Rig::start();
    let headers = excluding(r#"["node_modules","Pods"]"#);
    let args = json!({ "exclude": ["app/build"] });
    rig.call_with("sync", args, &headers).unwrap();
    let sent = rig.mirror_excludes.lock().unwrap().clone();
    assert_eq!(sent, [json!(["node_modules", "Pods", "app/build"])]);
}

#[test]
fn machine_info_shows_the_patterns_every_sync_leaves_alone() {
    let rig = Rig::start();
    let headers = excluding(r#"["node_modules","Pods"]"#);
    let info = rig.call_with("machine_info", json!({}), &headers).unwrap();
    assert_eq!(info["sync_exclude"], json!(["node_modules", "Pods"]));
    let info = rig.call("machine_info", json!({})).unwrap();
    assert_eq!(info["sync_exclude"], json!([]));
}

#[test]
fn a_bad_exclude_header_is_refused() {
    let rig = Rig::start();
    rig.write("a.txt", "a");
    for bad in ["node_modules", "[1]", r#"{"a":1}"#, r#"["-rf"]"#] {
        let err = rig
            .call_with("sync", json!({}), &excluding(bad))
            .unwrap_err();
        assert!(err.contains("X-Moor-Machine-Exclude"), "{bad}: {err}");
    }
    assert!(!rig.copy().join("a.txt").exists(), "nothing was synced");
}
