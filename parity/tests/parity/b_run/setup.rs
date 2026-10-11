use std::path::PathBuf;

use parity::adapter::{BoxState, ExitClass, StartOpts, TaskState};
use parity::assert::assert_order;
use parity::fake::crabbox;
use parity::parity;
use parity::world::World;
use serde_json::{json, Value};

use super::*;
use crate::moor;

fn start_again(f: &Fx) -> parity::adapter::Outcome<parity::adapter::TaskView> {
    moor().start(&f.w, &f.repo.path, &f.name, &StartOpts::default())
}

fn setup_cfg(steps: &[&str]) -> Option<Value> {
    Some(json!({ "setup": steps }))
}

parity!(
    provision_copies_and_links_into_the_worktree,
    covers = ["B-50"],
    tags = [],
    {
        let (w, repo) = prepare(
            Some(json!({
                "provision": [".env.local", "exists.txt", { "path": "cfg/secret.txt", "mode": "symlink" }],
                "setup": ["test -f .env.local && echo seen > .moor/prov-seen"],
            })),
            json!({}),
        );
        put(&repo.path, "exists.txt", "tracked");
        commit_all(&w, &repo.path, "add exists");
        put(&repo.path, "exists.txt", "host-copy");
        put(&repo.path, ".env.local", "TOKEN=1");
        put(&repo.path, "cfg/secret.txt", "s3");
        let (_name, wt, _slug) = start_task(&w, &repo, "prov");
        assert_eq!(
            std::fs::read_to_string(wt.join(".env.local")).unwrap(),
            "TOKEN=1"
        );
        assert_eq!(
            std::fs::read_to_string(wt.join("exists.txt")).unwrap(),
            "tracked"
        );
        let link = wt.join("cfg/secret.txt");
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(
            std::fs::canonicalize(&link).unwrap(),
            std::fs::canonicalize(repo.path.join("cfg/secret.txt")).unwrap()
        );
        assert!(
            wt.join(".moor/prov-seen").exists(),
            "setup sees provisioned files"
        );
    }
);

parity!(
    provision_missing_source_warns_and_skips_setup,
    covers = ["B-51"],
    tags = [],
    {
        let (w, repo) = prepare(
            Some(
                json!({ "provision": ["missing-a", "missing-b"], "setup": ["touch .moor/setup-ran"] }),
            ),
            json!({}),
        );
        let o = moor().start(&w, &repo.path, &w.task_name("pv"), &StartOpts::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().state, TaskState::Working);
        assert!(!o.warnings.is_empty(), "the failure is a warning");
        let wt = PathBuf::from(o.ok().worktree.clone().unwrap());
        assert!(!wt.join(".moor/setup-ran").exists(), "setup is skipped");
        assert!(!wt.join("missing-a").exists());
    }
);

parity!(
    setup_runs_synced_on_the_box_with_a_log,
    covers = ["B-52", "B-55"],
    tags = [],
    {
        let cfg = json!({
            "run": { "env": { "SETUP_ENV": "from-run-env" } },
            "setup": ["echo step-one-ran", "echo $SETUP_ENV > .moor/env-seen"],
        });
        let f = fx_with(Some(cfg));
        let runs = user_runs(&f.w);
        assert_eq!(runs.len(), 1, "setup is one Driver run");
        assert!(!has(&runs[0], "--no-sync") && has(&runs[0], "--no-hydrate"));
        assert_eq!(runs[0].flag("--id").as_deref(), Some(f.slug.as_str()));
        let log = std::fs::read_to_string(f.wt.join(".moor/setup.log")).unwrap();
        assert!(log.contains("step-one-ran"), "{log}");
        assert_eq!(
            std::fs::read_to_string(f.wt.join(".moor/setup.rc"))
                .unwrap()
                .trim(),
            "0"
        );
        let seen = std::fs::read_to_string(f.wt.join(".moor/env-seen")).unwrap();
        assert_eq!(seen.trim(), "from-run-env");
    }
);

parity!(
    setup_failure_is_not_fatal_and_stops_later_steps,
    covers = ["B-53"],
    tags = [],
    {
        let (w, repo) = prepare(
            setup_cfg(&["echo before", "false", "echo after-the-failure"]),
            json!({}),
        );
        let o = moor().start(&w, &repo.path, &w.task_name("sf"), &StartOpts::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let t = o.ok();
        assert_eq!(t.state, TaskState::Working);
        assert_eq!(t.boxed.as_ref().unwrap().state, BoxState::Ready);
        assert!(
            !o.warnings.is_empty(),
            "setup failure surfaces as a warning"
        );
        let wt = PathBuf::from(t.worktree.clone().unwrap());
        let log = std::fs::read_to_string(wt.join(".moor/setup.log")).unwrap();
        assert!(
            log.contains("before") && !log.contains("after-the-failure"),
            "{log}"
        );
        assert_ne!(
            std::fs::read_to_string(wt.join(".moor/setup.rc"))
                .unwrap()
                .trim(),
            "0"
        );
    }
);

parity!(
    start_rerun_after_setup_failure_reuses_warmed_box,
    covers = ["B-54"],
    tags = [],
    {
        let f = fx_with(setup_cfg(&["false"]));
        let cfg = json!({ "setup": ["echo fixed > .moor/fixed"] });
        std::fs::write(f.repo.path.join(".moor.yaml"), cfg.to_string()).unwrap();
        let again = start_again(&f);
        assert_eq!(again.class, ExitClass::Ok, "{}", again.stderr);
        assert_eq!(again.ok().state, TaskState::Working);
        assert!(again.warnings.is_empty(), "{:?}", again.warnings);
        assert_eq!(verb_calls(&f.w, "warmup").len(), 1);
    }
);

parity!(
    close_runs_teardown_before_release,
    covers = ["B-56"],
    tags = [],
    {
        let mark = |w: &World| w.box_root().join("teardown.mark");
        let probe_cfg = |w: &World| {
            Some(json!({ "teardown": [format!("echo bye > '{}'", mark(w).display())] }))
        };
        let (w, repo) = prepare(None, json!({}));
        std::fs::write(
            repo.path.join(".moor.yaml"),
            probe_cfg(&w).unwrap().to_string(),
        )
        .unwrap();
        let (name, _wt, _slug) = start_task(&w, &repo, "td");
        let closed = moor().close(&w, &repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        assert!(closed.ok().box_deleted);
        assert!(mark(&w).exists(), "teardown ran on the Box");
        assert_order(
            &w.all_calls(),
            &[("crabbox", &["run"]), ("crabbox", &["stop"])],
        );
        let last_run = user_runs(&w).pop().unwrap();
        assert!(!has(&last_run, "--no-sync"));
    }
);

parity!(
    close_teardown_failure_still_releases_the_box,
    covers = ["B-56"],
    tags = [],
    {
        let (w, repo) = prepare(Some(json!({ "teardown": ["false"] })), json!({}));
        let (name, _wt, _slug) = start_task(&w, &repo, "tf");
        let closed = moor().close(&w, &repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        assert!(closed.ok().box_deleted);
        assert_eq!(verb_calls(&w, "stop").len(), 1);
    }
);

parity!(
    close_skips_teardown_when_the_box_is_gone,
    covers = ["B-56"],
    tags = [],
    {
        let (w, repo) = prepare(None, json!({}));
        let mark = w.box_root().join("teardown.mark");
        let cfg = json!({ "teardown": [format!("echo bye > '{}'", mark.display())] });
        std::fs::write(repo.path.join(".moor.yaml"), cfg.to_string()).unwrap();
        let (name, _wt, slug) = start_task(&w, &repo, "tg");
        crabbox::gone(&w, &slug);
        moor().list(&w, &repo.path).ok();
        moor().close(&w, &repo.path, &name, false);
        assert!(!mark.exists());
        assert!(verb_calls(&w, "run").is_empty());
    }
);
