use parity::adapter::{BoxState, EventKind, ExitClass, RunOpts, StartOpts, TaskState};
use parity::assert::assert_order;
use parity::fake::crabbox;
use parity::parity;
use serde_json::json;

use super::*;
use crate::moor;

parity!(
    run_exit_code_and_output_propagate,
    covers = ["B-01", "B-05", "B-11", "REQ-23", "REQ-25"],
    tags = [],
    {
        let f = fx();
        let o = go(&f, &["sh", "-c", "echo parity-out; exit 7"]);
        assert_eq!(o.exit, 7, "remote exit code reaches the caller");
        assert_eq!(o.class, ExitClass::Failed);
        assert!(o.stdout.contains("parity-out"), "{}", o.stdout);
        let r = o.value.as_ref().expect("run result");
        assert_eq!((r.exit, r.interrupted), (7, false));
        let ok = go(&f, &["true"]);
        assert_eq!(ok.class, ExitClass::Ok, "{}", ok.stderr);
        assert_eq!(ran_events(&f.w, &f.name), 2);
        let runs = user_runs(&f.w);
        assert_eq!(runs.len(), 2);
        let c = &runs[0];
        assert_eq!(c.flag("--provider").as_deref(), Some("local-container"));
        assert_eq!(c.flag("--id").as_deref(), Some(f.slug.as_str()));
        assert!(has(c, "--no-hydrate") && !has(c, "--no-sync"));
        assert_eq!(
            c.env
                .get("CRABBOX_SYNC_GIT_SEED_SOURCE")
                .map(String::as_str),
            Some("local")
        );
        assert_eq!(
            std::fs::canonicalize(&c.cwd).ok(),
            std::fs::canonicalize(&f.wt).ok()
        );
    }
);

parity!(
    run_command_quoting_and_shell,
    covers = ["B-02", "B-03", "B-18", "REQ-23"],
    tags = [],
    {
        let f = fx();
        let multi = go(&f, &["printf", "%s|%s", "a b", "it's"]);
        assert_eq!(multi.class, ExitClass::Ok, "{}", multi.stderr);
        assert_eq!(multi.stdout, "a b|it's");
        let one = go(&f, &["echo one; echo two | tr a-z A-Z"]);
        assert!(one.stdout.contains("one") && one.stdout.contains("TWO"));
        let pf = go(&f, &["set -o pipefail; false | true; echo rc=$?"]);
        assert!(pf.stdout.contains("rc=1"), "{} {}", pf.stdout, pf.stderr);
        let plain = go(&f, &["false | true; echo rc=$?"]);
        assert_eq!(plain.class, ExitClass::Ok);
        assert!(plain.stdout.contains("rc=0"));
    }
);

parity!(
    run_sync_flag_follows_no_sync,
    covers = ["B-04", "REQ-24"],
    tags = [],
    {
        let f = fx();
        go(&f, &["true"]);
        let quick = moor().run(
            &f.w,
            &f.repo.path,
            &f.name,
            &["true"],
            &RunOpts {
                no_sync: true,
                ..Default::default()
            },
        );
        assert_eq!(quick.class, ExitClass::Ok, "{}", quick.stderr);
        let runs = user_runs(&f.w);
        assert!(!has(&runs[0], "--no-sync"));
        assert!(has(&runs[1], "--no-sync"));
        assert!(runs.iter().all(|c| has(c, "--no-hydrate")));
        assert_eq!(ran_events(&f.w, &f.name), 2);
    }
);

parity!(
    run_size_maps_to_class,
    covers = ["B-07", "REQ-30"],
    tags = [],
    {
        let cases = [
            (None, "standard"),
            (Some("medium"), "fast"),
            (Some("large"), "large"),
            (Some("xlarge"), "beast"),
        ];
        for (size, class) in cases {
            let cfg = size.map(|s| json!({ "box": { "size": s } }));
            let f = fx_with(cfg);
            let o = go(&f, &["true"]);
            assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
            let run = user_runs(&f.w).remove(0);
            assert_eq!(run.flag("--class").as_deref(), Some(class), "size {size:?}");
            let warm = verb_calls(&f.w, "warmup").remove(0);
            assert_eq!(warm.flag("--class").as_deref(), Some(class));
        }
    }
);

parity!(
    run_applies_repo_run_env_and_path,
    covers = ["B-08"],
    tags = [],
    {
        let cfg =
            json!({ "run": { "env": { "PARITY_FOO": "bar baz" }, "path": ["/opt/parity-x"] } });
        let f = fx_with(Some(cfg));
        let o = go(&f, &["printf '%s|%s' \"$PARITY_FOO\" \"$PATH\""]);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(
            o.stdout.starts_with("bar baz|/opt/parity-x:"),
            "{}",
            o.stdout
        );
    }
);

parity!(
    run_refused_without_a_ready_box,
    covers = ["B-09", "B-10"],
    tags = [],
    {
        let f = fx();
        let m = moor();
        let before = user_runs(&f.w).len();
        let none = m.run(&f.w, &f.repo.path, "nope", &["true"], &RunOpts::default());
        assert!(none.is_err());
        let empty = m.run(&f.w, &f.repo.path, &f.name, &[], &RunOpts::default());
        assert!(empty.is_err());
        assert_eq!(user_runs(&f.w).len(), before);

        crabbox::gone(&f.w, &f.slug);
        let listed = m.list(&f.w, &f.repo.path);
        let row = listed.ok().iter().find(|t| t.name == f.name).cloned();
        assert_eq!(row.unwrap().boxed.unwrap().state, BoxState::Gone);
        let gone = go(&f, &["true"]);
        assert!(gone.is_err());
        assert!(gone.value.is_none());
        assert_eq!(user_runs(&f.w).len(), before);
        let kinds: Vec<_> = m
            .events(&f.w, &f.name)
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert!(
            kinds.contains(&EventKind::Ran),
            "waiters must see a ran event"
        );
    }
);

parity!(
    run_refused_while_waiting_for_a_box,
    covers = ["B-10"],
    tags = [],
    {
        let (w, repo) = prepare(None, json!({ "max_boxes": 1 }));
        let (_first, _wt, _slug) = start_task(&w, &repo, "cap");
        let second = w.task_name("wait");
        let o = moor().start(&w, &repo.path, &second, &StartOpts::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().state, TaskState::WaitingForBox);
        let r = moor().run(&w, &repo.path, &second, &["true"], &RunOpts::default());
        assert!(r.is_err());
        assert!(user_runs(&w).is_empty());
    }
);

parity!(
    run_quiet_keeps_program_output_and_exit,
    covers = [],
    tags = [],
    {
        let f = fx();
        let o = moor().run(
            &f.w,
            &f.repo.path,
            &f.name,
            &["sh", "-c", "echo quiet-out; exit 3"],
            &RunOpts {
                quiet: true,
                ..Default::default()
            },
        );
        assert_eq!(o.exit, 3);
        assert!(o.stdout.contains("quiet-out"), "{}", o.stdout);
    }
);

parity!(
    run_interrupt_marks_run_interrupted,
    covers = ["B-12", "REQ-23"],
    tags = [],
    {
        let f = fx();
        crabbox::block(&f.w, "run");
        let o = moor().run(
            &f.w,
            &f.repo.path,
            &f.name,
            &["sleep", "600"],
            &RunOpts {
                interrupt: true,
                ..Default::default()
            },
        );
        assert!(o.is_err(), "interrupted run is not a success");
        assert_eq!(o.exit, 130);
        let r = o.value.as_ref().expect("a ran event records the interrupt");
        assert!(r.interrupted);
        assert_eq!(ran_events(&f.w, &f.name), 1);
        assert_order(&f.w.all_calls(), &[("crabbox", &["run"])]);
    }
);

parity!(
    run_child_killed_by_signal_is_interrupted,
    covers = ["B-12"],
    tags = [],
    {
        let f = fx();
        crabbox::die_on_run(&f.w);
        let o = go(&f, &["true"]);
        assert!(o.is_err());
        assert_eq!(o.exit, 130);
        assert!(o.value.as_ref().expect("ran event").interrupted);
    }
);
