use std::path::Path;

use parity::adapter::{ExitClass, RunOpts, StartOpts};
use parity::fake::crabbox;
use parity::parity;
use serde_json::json;

use super::*;
use crate::moor;

fn gate(
    f: &Fx,
    cwd: &Path,
    sha: &str,
    name: &str,
) -> parity::adapter::Outcome<parity::adapter::GateVerdict> {
    moor().gate_check(&f.w, cwd, sha, name, None)
}

parity!(
    record_runs_from_clean_checkout_and_stores_result,
    covers = ["B-31", "B-32", "B-40", "REQ-26"],
    tags = [],
    {
        let f = fx();
        let head = wt_commit(&f, &[(".gitignore", "junk.log\n")]);
        put(&f.wt, "junk.log", "stale");
        let cmd = "test ! -e junk.log && echo clean-start";
        let o = rec(&f, "unit", cmd);
        assert_eq!(o.class, ExitClass::Ok, "{} {}", o.stdout, o.stderr);
        assert!(o.stdout.contains("clean-start"));
        let r = o.value.as_ref().expect("run result");
        assert_eq!(r.sha.as_deref(), Some(head.as_str()));
        let rows = moor().runs(&f.w, &f.name);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].name.as_str(), rows[0].command.as_str()),
            ("unit", cmd)
        );
        assert_eq!((rows[0].exit, rows[0].interrupted), (0, false));
        assert_eq!(rows[0].sha, head);
        let g = gate(&f, &f.w.tmp_dir(), &head[..7], "unit");
        assert_eq!(g.class, ExitClass::Ok, "{}", g.stderr);
        assert!(g.ok().ok);
    }
);

parity!(
    record_failure_is_stored_and_fails_the_gate,
    covers = ["B-32", "B-36"],
    tags = [],
    {
        let f = fx();
        let head = wt_commit(&f, &[("a.txt", "a")]);
        let o = rec(&f, "unit", "exit 5");
        assert_eq!(o.exit, 5);
        assert_eq!(o.value.as_ref().unwrap().exit, 5);
        let rows = moor().runs(&f.w, &f.name);
        assert_eq!((rows[0].exit, rows[0].interrupted), (5, false));
        let g = gate(&f, &f.w.tmp_dir(), &head, "unit");
        assert_eq!(g.class, ExitClass::Failed);
        let v = g.value.as_ref().expect("verdict json on failure");
        assert!(!v.ok);
        assert_eq!(v.exit, Some(5));
        let none = gate(&f, &f.w.tmp_dir(), &head, "other");
        assert_eq!(none.class, ExitClass::Failed);
        assert_eq!(
            none.value.as_ref().map(|v| (v.ok, v.exit)),
            Some((false, None))
        );
    }
);

parity!(
    record_interrupted_run_fails_the_gate,
    covers = ["B-36"],
    tags = [],
    {
        let f = fx();
        let head = wt_commit(&f, &[("a.txt", "a")]);
        crabbox::block(&f.w, "run");
        let o = moor().run(
            &f.w,
            &f.repo.path,
            &f.name,
            &["sleep 600"],
            &RunOpts {
                record: Some("unit".into()),
                interrupt: true,
                ..Default::default()
            },
        );
        assert!(o.value.as_ref().expect("ran event").interrupted);
        let rows = moor().runs(&f.w, &f.name);
        assert!(rows[0].interrupted);
        let g = gate(&f, &f.w.tmp_dir(), &head, "unit");
        assert_eq!(g.class, ExitClass::Failed);
        assert!(!g.value.as_ref().unwrap().ok);
    }
);

parity!(
    record_refuses_unsafe_trees_before_any_box_call,
    covers = ["B-30"],
    tags = [],
    {
        let f = fx();
        wt_commit(&f, &[(".gitignore", "ignored.txt\n")]);
        let m = moor();
        put(&f.wt, "ignored.txt", "x");
        put(&f.wt, "untracked.txt", "x");
        let dirty = rec(&f, "unit", "true");
        assert!(dirty.is_err(), "untracked file refuses --record");
        std::fs::remove_file(f.wt.join("untracked.txt")).unwrap();
        let no_sync = m.run(
            &f.w,
            &f.repo.path,
            &f.name,
            &["true"],
            &RunOpts {
                record: Some("unit".into()),
                no_sync: true,
                ..Default::default()
            },
        );
        assert!(no_sync.is_err(), "--record needs sync");
        assert!(
            user_runs(&f.w).is_empty(),
            "no Box call for a refused record"
        );
        std::fs::remove_dir_all(&f.wt).unwrap();
        let gone = rec(&f, "unit", "true");
        assert!(gone.is_err(), "missing worktree refuses --record");
        assert!(user_runs(&f.w).is_empty());
        assert!(m.runs(&f.w, &f.name).is_empty());
    }
);

parity!(
    gate_check_sha_forms,
    covers = ["B-34", "REQ-28"],
    tags = [],
    {
        let f = fx();
        let head = wt_commit(&f, &[("a.txt", "a")]);
        assert_eq!(rec(&f, "unit", "true").class, ExitClass::Ok);
        let away = f.w.tmp_dir();
        let short = &head[..7];
        assert!(gate(&f, &away, short, "unit").ok().ok);
        assert!(gate(&f, &away, &short.to_uppercase(), "unit").ok().ok);
        assert!(gate(&f, &away, &head, "unit").ok().ok);
        for bad in ["abc", "zzzz", ""] {
            let o = gate(&f, &away, bad, "unit");
            assert_eq!(o.class, ExitClass::Usage, "sha {bad:?}");
        }
        let unknown_short = gate(&f, &away, "deadb", "unit");
        assert_eq!(unknown_short.class, ExitClass::Usage);
        let unknown_long = gate(&f, &away, "deadbee", "unit");
        assert_eq!(unknown_long.class, ExitClass::Failed);
    }
);

parity!(gate_check_matching_rules, covers = ["B-35"], tags = [], {
    let f = fx();
    let head = wt_commit(&f, &[("a.txt", "a")]);
    assert_eq!(rec(&f, "unit", "echo first").class, ExitClass::Ok);
    assert_eq!(rec(&f, "unit", "exit 4").exit, 4);
    let away = f.w.tmp_dir();
    assert!(
        gate(&f, &away, &head, "unit").ok().ok,
        "an older pass beats a newer fail"
    );
    let m = moor();
    let by_cmd = |c: &str| m.gate_check(&f.w, &away, &head, "unit", Some(c));
    assert!(by_cmd("echo first").ok().ok);
    assert_eq!(by_cmd("exit 4").class, ExitClass::Failed);
    assert_eq!(by_cmd("echo other").class, ExitClass::Failed);
    for tail in ["1111", "2222"] {
        m.seed_run(
            &f.w,
            &parity::adapter::RunRow {
                task: f.name.clone(),
                sha: format!("abcdef0{tail}{}", "0".repeat(29)),
                name: "amb".into(),
                command: "true".into(),
                exit: 0,
                interrupted: false,
            },
        );
    }
    let amb = gate(&f, &away, "abcdef0", "amb");
    assert_eq!(
        amb.class,
        ExitClass::Usage,
        "ambiguous prefix is a usage error"
    );
    assert!(amb.value.is_none_or(|v| !v.ok));
    let exact = gate(&f, &away, &format!("abcdef01111{}", "0".repeat(29)), "amb");
    assert!(exact.ok().ok);
});

parity!(
    repo_config_errors_block_start,
    covers = ["B-38", "B-57"],
    tags = [],
    {
        let docs = [
            (
                json!({ "record": { "keep": ["node_modules"] } }),
                "record.keep",
            ),
            (json!({ "setup": [""] }), "setup"),
            (json!({ "teardown": ["  "] }), "teardown"),
            (json!({ "tools": [""] }), "tools"),
            (json!({ "provision": ["../outside"] }), "provision"),
            (json!({ "provision": ["/etc/hostname"] }), "provision"),
        ];
        for (doc, key) in docs {
            let (w, repo) = prepare(Some(doc.clone()), json!({}));
            let o = moor().start(&w, &repo.path, &w.task_name("cfg"), &StartOpts::default());
            assert_eq!(o.class, ExitClass::Failed, "{doc}: {}", o.stderr);
            assert!(o.stderr.contains(key), "{doc}: {}", o.stderr);
            assert!(verb_calls(&w, "warmup").is_empty(), "{doc}");
        }
    }
);
