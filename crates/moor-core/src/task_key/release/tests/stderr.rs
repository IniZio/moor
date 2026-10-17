//! The generated scripts run under `exec 2>&1`, so a diagnostic from `awk` or `ps` while reading
//! the process snapshot would land in the captured output. The snapshot holds environment text
//! that may be secret, so no such diagnostic may reach it.

use std::fs;
use std::process::Command;

use super::{
    key, kill_tree_script, layout, orphan_ibtoold_script, path_pids_cmd, snapshot_cmd,
    stop_agent_script, tagged_pids_cmd, NONCE,
};

const LEAK: &str = "SECRET=hunter2";

/// Runs `script` under `sh` with `exec 2>&1`, a canned `moor_ps`, a pass-through `moor_confirm`,
/// and `awk` and `ps` stubs that print a diagnostic quoting [`LEAK`] to stderr before running the
/// real tool. Returns the combined output and whether both stubs ran.
fn run_noisy(name: &str, rows: &str, script: &str) -> (String, bool) {
    let dir = std::env::temp_dir().join(format!("moor-stderr-{name}-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let mut real = Vec::new();
    for tool in ["awk", "ps"] {
        let found = Command::new("sh")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .unwrap();
        real.push(String::from_utf8(found.stdout).unwrap().trim().to_owned());
    }
    for (tool, path) in ["awk", "ps"].iter().zip(&real) {
        let stub = dir.join(tool);
        fs::write(
            &stub,
            format!(
                "#!/bin/sh\necho \"{tool}: weird input record '{LEAK}'\" >&2\ntouch '{}'\nexec {path} \"$@\"\n",
                dir.join(format!("{tool}.ran")).display()
            ),
        )
        .unwrap();
        Command::new("chmod").arg("+x").arg(&stub).status().unwrap();
    }
    let full = format!(
        "exec 2>&1\nmoor_confirm() {{ cat; }}\nmoor_ps() {{ cat <<'MOOR_EOF'\n{rows}\nMOOR_EOF\n}}\n{script}"
    );
    let old = std::env::var("PATH").unwrap();
    let out = Command::new("sh")
        .arg("-c")
        .arg(full)
        .env("PATH", format!("{}:{old}", dir.display()))
        .output()
        .unwrap();
    let ran = dir.join("awk.ran").exists() && dir.join("ps.ran").exists();
    fs::remove_dir_all(&dir).unwrap();
    (String::from_utf8(out.stdout).unwrap(), ran)
}

fn assert_quiet(name: &str, rows: &str, script: &str) {
    let (out, ran) = run_noisy(name, rows, script);
    assert!(ran, "{name}: stubs did not run");
    assert!(!out.contains(LEAK), "{name}: leaked secret: {out}");
    assert!(!out.contains("awk:"), "{name}: awk diagnostic: {out}");
    assert!(!out.contains("ps:"), "{name}: ps diagnostic: {out}");
}

fn rows() -> String {
    [
        "2147001 2147002 2147001 s9 /usr/bin/nono run HOME=/h".to_owned(),
        format!("2147001 2147002 2147001 s9 /usr/bin/nono run MOOR_BOX_TAG=app-{NONCE}"),
        "2147002 1 2147002 s9 /bin/sh /w/app/work/run.sh".to_owned(),
        "2147003 1 2147003 s9 /usr/bin/ibtoold /w/other".to_owned(),
    ]
    .join("\n")
}

#[test]
fn kill_tree_keeps_awk_and_ps_stderr_out() {
    let script = kill_tree_script(&key("app"), &layout("app"), &[]);
    assert_quiet("kill", &rows(), &script);
}

#[test]
fn matchers_keep_awk_stderr_out() {
    let script = format!(
        "{}{}\n{}\n",
        snapshot_cmd(),
        tagged_pids_cmd(&key("app")),
        path_pids_cmd(&layout("app"))
    );
    assert_quiet("match", &rows(), &script);
}

#[test]
fn stop_agent_keeps_awk_stderr_out() {
    let script = stop_agent_script(&key("app"), &layout("app"), 0);
    assert_quiet("stop", &rows(), &script);
}

#[test]
fn orphan_ibtoold_keeps_awk_stderr_out() {
    assert_quiet("ibtoold", &rows(), &orphan_ibtoold_script(&layout("app")));
}
