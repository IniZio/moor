use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::fake::Call;
use crate::world::World;

fn sorted(calls: &[Call]) -> Vec<&Call> {
    let mut v: Vec<&Call> = calls.iter().collect();
    v.sort_by_key(|c| c.seq);
    v
}

fn listing(calls: &[Call]) -> String {
    if calls.is_empty() {
        return "  (no calls recorded)".to_string();
    }
    sorted(calls)
        .iter()
        .map(|c| {
            format!("  {} {} {}", c.seq, c.tool, c.argv.join(" "))
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn hit(c: &Call, tool: &str, prefix: &[&str]) -> bool {
    c.tool == tool && c.argv.len() >= prefix.len() && c.argv.iter().zip(prefix).all(|(a, p)| a == p)
}

fn describe(tool: &str, prefix: &[&str]) -> String {
    format!("{} {}", tool, prefix.join(" "))
        .trim_end()
        .to_string()
}

pub fn assert_order(calls: &[Call], expected: &[(&str, &[&str])]) {
    let ordered = sorted(calls);
    let mut at = 0;
    for (i, (tool, prefix)) in expected.iter().enumerate() {
        match ordered[at..].iter().position(|c| hit(c, tool, prefix)) {
            Some(p) => at += p + 1,
            None => panic!(
                "expectation #{} not met: `{}` not found in order after position {}\nrecorded calls:\n{}",
                i + 1,
                describe(tool, prefix),
                at,
                listing(calls)
            ),
        }
    }
}

pub fn assert_called(calls: &[Call], tool: &str, prefix: &[&str]) {
    if !calls.iter().any(|c| hit(c, tool, prefix)) {
        panic!(
            "expected call `{}` was never made\nrecorded calls:\n{}",
            describe(tool, prefix),
            listing(calls)
        );
    }
}

pub fn assert_never_called(calls: &[Call], tool: &str, prefix: &[&str]) {
    if let Some(c) = sorted(calls).into_iter().find(|c| hit(c, tool, prefix)) {
        panic!(
            "call `{}` must never happen but seq {} did\nrecorded calls:\n{}",
            describe(tool, prefix),
            c.seq,
            listing(calls)
        );
    }
}

pub fn assert_seq_global(calls: &[Call]) {
    let seqs: Vec<u64> = sorted(calls).iter().map(|c| c.seq).collect();
    let want: Vec<u64> = (1..=seqs.len() as u64).collect();
    if seqs != want {
        panic!(
            "seqs are not exactly 1..={}: got {:?}\nrecorded calls:\n{}",
            seqs.len(),
            seqs,
            listing(calls)
        );
    }
}

// PT4 assertions
pub fn assert_pure_json(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout)
        .unwrap_or_else(|e| panic!("stdout is not exactly one JSON document ({e}): {stdout}"))
}

fn has(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => out.extend(files_under(&path)),
            Ok(t) if t.is_file() => out.push(path),
            _ => {}
        }
    }
    out.sort();
    out
}

/// Fails when `secret` appears anywhere moor or a fake could have exposed it:
/// fake argv/env, moor call output, `state/` files (except 0600 files under an
/// `allow` prefix), `registry.db`, the `box/` tree, and `ps` output.
/// `secret` must be registered via `World::register_secret` before moor runs:
/// the fake flags calls whose env contains any registered secret as a
/// substring, so a wrapped secret in env fails every `assert_secret_absent`.
pub fn assert_secret_absent(w: &World, secret: &str, allow: &[&str]) {
    assert!(
        w.secrets().iter().any(|s| s == secret),
        "register_secret before running moor: the fake can only flag wrapped secrets it knows"
    );
    let needle = secret.as_bytes();
    let mut leaks: Vec<String> = Vec::new();
    let want = crate::fake::digest(secret);
    for c in crate::fake::all_calls(&w.fakes_dir()) {
        let in_argv = c.argv.iter().any(|a| a.contains(secret));
        let in_env = c.env.values().any(|v| v.contains(secret))
            || c.env_digests.values().any(|d| *d == want);
        if in_argv || in_env {
            let place = if in_argv { "argv" } else { "env" };
            leaks.push(format!("fake call {} {} ({place})", c.seq, c.tool));
        } else if c.env_contains_secret {
            leaks.push(format!("fake call {} {} (env, wrapped)", c.seq, c.tool));
        }
    }
    for (i, entry) in w.transcript().iter().enumerate() {
        if entry.contains(secret) {
            let head = entry.lines().next().unwrap_or("");
            leaks.push(format!("moor call #{i} output: {head}"));
        }
    }
    let state = w.state_dir();
    for f in files_under(&state) {
        let rel = f
            .strip_prefix(&state)
            .unwrap_or(&f)
            .to_string_lossy()
            .into_owned();
        let allowed = allow.iter().any(|a| rel.starts_with(a));
        let mode = std::fs::metadata(&f).map_or(0, |m| m.permissions().mode() & 0o777);
        if allowed && mode != 0o600 {
            leaks.push(format!(
                "allow-listed state/{rel} has mode {mode:o}, not 600"
            ));
        }
        let bytes = std::fs::read(&f).unwrap_or_default();
        if !allowed && has(&bytes, needle) {
            leaks.push(format!("state/{rel}"));
        }
    }
    for f in files_under(&w.box_root()) {
        if has(&std::fs::read(&f).unwrap_or_default(), needle) {
            let rel = f
                .strip_prefix(w.root())
                .unwrap_or(&f)
                .to_string_lossy()
                .into_owned();
            leaks.push(rel);
        }
    }
    let ps = std::process::Command::new("ps")
        .args(["-eww", "-o", "args="])
        .output()
        .map(|o| o.stdout)
        .unwrap_or_default();
    if has(&ps, needle) {
        leaks.push("ps output".to_string());
    }
    assert!(
        leaks.is_empty(),
        "secret leaked into:\n  {}",
        leaks.join("\n  ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn call(seq: u64, tool: &str, argv: &[&str]) -> Call {
        Call {
            seq,
            tool: tool.into(),
            argv: argv.iter().map(|s| s.to_string()).collect(),
            cwd: String::new(),
            pid: 0,
            ppid: 0,
            pgid: 0,
            starttime: 0,
            stdin: String::new(),
            env: BTreeMap::new(),
            env_digests: BTreeMap::new(),
            env_contains_secret: false,
            rule: None,
        }
    }

    fn sample() -> Vec<Call> {
        vec![
            call(3, "crabbox", &["run", "--", "ls"]),
            call(1, "herdr", &["worktree", "create"]),
            call(2, "crabbox", &["warmup", "--slug", "x"]),
        ]
    }

    #[test]
    #[should_panic(expected = "(env)")]
    fn secret_in_unprefixed_env_is_caught() {
        let w = World::new("/bin/true");
        w.register_secret("s3cr3t-value");
        let mut c = call(1, "claude", &[]);
        c.env_digests
            .insert("LEAKPROBE".into(), crate::fake::digest("s3cr3t-value"));
        let line = format!("{}\n", serde_json::to_string(&c).unwrap());
        std::fs::write(crate::fake::calls_path(&w.fakes_dir(), "claude"), line).unwrap();
        assert_secret_absent(&w, "s3cr3t-value", &[]);
    }

    #[test]
    fn assert_order_accepts_subsequence() {
        assert_order(
            &sample(),
            &[("herdr", &["worktree"]), ("crabbox", &["run"])],
        );
    }

    #[test]
    #[should_panic(expected = "expectation #2 not met")]
    fn assert_order_rejects_wrong_order() {
        assert_order(
            &sample(),
            &[("crabbox", &["run"]), ("herdr", &["worktree"])],
        );
    }

    #[test]
    #[should_panic(expected = "1 herdr worktree create")]
    fn assert_order_failure_lists_calls() {
        assert_order(&sample(), &[("claude", &["stop"])]);
    }

    #[test]
    fn called_and_never_called() {
        assert_called(&sample(), "crabbox", &["warmup"]);
        assert_never_called(&sample(), "crabbox", &["stop"]);
        assert_seq_global(&sample());
    }

    #[test]
    #[should_panic(expected = "must never happen")]
    fn never_called_fails_on_hit() {
        assert_never_called(&sample(), "herdr", &["worktree"]);
    }

    #[test]
    #[should_panic(expected = "not exactly")]
    fn seq_gap_fails() {
        assert_seq_global(&[call(1, "a", &[]), call(3, "b", &[])]);
    }
}
