use crate::fake::Call;

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
        assert_order(&sample(), &[("claude", &["agents"])]);
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
