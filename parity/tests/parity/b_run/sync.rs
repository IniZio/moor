use std::path::Path;

use parity::adapter::{ExitClass, RunOpts};
use parity::assert::{assert_never_called, assert_order};
use parity::parity;
use parity::world::repo::Repo;
use parity::world::World;
use serde_json::json;

use super::*;
use crate::moor;

parity!(
    runs_that_sync_and_runs_that_do_not,
    covers = ["B-60"],
    tags = [],
    {
        let f = fx_with(Some(json!({ "setup": ["true"], "teardown": ["true"] })));
        go(&f, &["true"]);
        seed_box_file(&f, "rel.txt", "x");
        let cwd = host_dir(&f, "sync-cp");
        let cp = moor().cp(&f.w, &cwd, &f.name, "rel.txt", Path::new("o"));
        assert_eq!(cp.class, ExitClass::Ok, "{}", cp.stderr);
        let closed = moor().close(&f.w, &f.repo.path, &f.name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        let all = verb_calls(&f.w, "run");
        let (probes, real): (Vec<_>, Vec<_>) = all
            .iter()
            .partition(|c| c.argv.iter().any(|a| a.contains("MOOR_WORKDIR")));
        let synced = real.iter().filter(|c| !has(c, "--no-sync")).count();
        assert_eq!(synced, 3, "setup, run and teardown sync: {real:?}");
        assert_eq!(probes.len(), 1);
        assert!(has(probes[0], "--no-sync"));
        assert!(verb_calls(&f.w, "cp").iter().all(|c| !has(c, "--no-sync")));
    }
);

parity!(
    box_scratch_dirs_do_not_dirty_the_worktree,
    covers = ["B-65"],
    tags = [],
    {
        let f = fx();
        assert_eq!(
            go(
                &f,
                &["mkdir -p .moor .crabbox && echo x > .moor/a && echo y > .crabbox/b"]
            )
            .class,
            ExitClass::Ok
        );
        let status = git_out(&f.w, &f.wt, &["status", "--porcelain"]);
        assert_eq!(status, "", "worktree stays clean");
        let again = git_out(&f.w, &f.repo.path, &["status", "--porcelain"]);
        assert!(
            !again.contains(".moor/") && !again.contains(".crabbox/"),
            "{again}"
        );
    }
);

fn box_cred_world(secret: &str) -> (World, Repo) {
    let (w, repo) = prepare(
        Some(json!({ "box_credentials": { "TOK": { "env": "PARITY_TOK" } } })),
        json!({}),
    );
    let user = json!({
        "credentials": { "TOK": { "cmd": format!("printf %s {secret}") } },
        "drivers": { "crabbox": {} },
    });
    std::fs::write(w.config_path(), user.to_string()).unwrap();
    parity::fake::ssh::install(&w);
    let ssh = format!("{} fake-host\n", w.tool_path("ssh").display());
    w.prepend_rules(
        "crabbox",
        [
            parity::fake::Rule::on(&["ssh"]).stdout(ssh),
            parity::fake::Rule::on(&["run"])
                .exec("{post}")
                .rewrite("/tmp/moor-cred-", "{box}/tmp/moor-cred-"),
        ],
    );
    (w, repo)
}

parity!(
    run_delivers_box_credentials_over_ssh_and_cleans_up,
    covers = ["B-16", "REQ-31"],
    tags = [],
    {
        let secret = "PARITY-SECRET-7788";
        let (mut w, repo) = box_cred_world(secret);
        w.watch_creds_in(Path::new("/tmp"));
        let (name, _wt, _slug) = start_task(&w, &repo, "bc");
        let o = moor().run(
            &w,
            &repo.path,
            &name,
            &["printf '%s' \"$PARITY_TOK\""],
            &RunOpts::default(),
        );
        assert_eq!(o.class, ExitClass::Ok, "{} {}", o.stdout, o.stderr);
        assert_eq!(o.stdout, secret, "the Box command sees the credential");
        assert_order(
            &w.all_calls(),
            &[("crabbox", &["ssh"]), ("ssh", &[]), ("crabbox", &["run"])],
        );
        let leftovers: Vec<_> = std::fs::read_dir(w.box_root().join("tmp"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("moor-cred-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "credential dir removed: {leftovers:?}"
        );
        for c in w.calls("crabbox") {
            let hay = format!("{:?} {:?}", c.argv, c.env);
            assert!(
                !hay.contains(secret),
                "secret leaked into crabbox call: {hay}"
            );
        }
        w.assert_no_new_tmp_creds();
    }
);

parity!(
    run_with_missing_box_credential_source_fails_before_delivery,
    covers = ["B-16"],
    tags = [],
    {
        let (w, repo) = box_cred_world("x");
        let (name, _wt, _slug) = start_task(&w, &repo, "bm");
        std::fs::write(
            w.config_path(),
            json!({ "drivers": { "crabbox": {} } }).to_string(),
        )
        .unwrap();
        let o = moor().run(&w, &repo.path, &name, &["true"], &RunOpts::default());
        assert!(o.is_err());
        assert_never_called(&w.all_calls(), "ssh", &[]);
        assert!(user_runs(&w).is_empty());
    }
);
