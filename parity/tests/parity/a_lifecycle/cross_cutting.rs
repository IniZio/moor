use parity::adapter::{ExitClass, StartOpts};
use parity::fake::Rule;
use parity::parity;
use serde_json::json;

use super::support::*;

parity!(
    credential_values_stay_out_of_outputs_registry_logs_and_driver_argv,
    covers = ["A-102", "REQ-31"],
    tags = [],
    {
        let f = fx();
        let (host, mcp) = ("FAKE-SECRET-4242", "s3cr3t-MCP-77");
        write_json(
            &f.w.config_path(),
            json!({
                "credentials": {
                    "TOKEN": { "cmd": "printf FAKE-SECRET-%s 4242" },
                    "PROBE_TOKEN": { "cmd": format!("echo {mcp}") }
                },
                "mcp_servers": { "probe": {
                    "command": "python3", "args": ["server.py"],
                    "env": { "PROBE_TOKEN": "${PROBE_TOKEN}" }
                } }
            }),
        );
        f.repo_cfg(json!({ "credentials": ["TOKEN"], "setup": ["echo ok"] }));
        let name = f.name("leak");
        let started = f.try_start(
            &name,
            &StartOpts {
                goal: Some("go".into()),
                ..Default::default()
            },
        );
        assert_eq!(started.class, ExitClass::Ok, "{}", started.stderr);
        assert_eq!(super::box_setup::agent_launches(&f), 1);
        let listed = f.m.list(&f.w, &f.repo.path);
        let mut outputs = vec![started.stdout, started.stderr, listed.stdout, listed.stderr];
        for secret in [host, mcp] {
            for o in &outputs {
                assert!(!o.contains(secret), "{secret} in output");
            }
            for p in files_under(&f.w.state_dir()) {
                assert!(
                    !contains(&std::fs::read(&p).unwrap(), secret),
                    "{}",
                    p.display()
                );
            }
            for c in f.w.all_calls() {
                let text = format!("{} {} {:?}", c.tool, c.argv.join(" "), c.env);
                assert!(!text.contains(secret), "{secret} in {text}");
            }
        }
        let closed = f.try_close(&name);
        outputs.push(closed.stdout);
        outputs.push(closed.stderr);
        assert!(outputs
            .iter()
            .all(|o| !o.contains(host) && !o.contains(mcp)));
    }
);

parity!(
    one_live_task_per_name_and_events_are_append_only,
    covers = ["A-104"],
    tags = [],
    {
        let f = fx();
        let name = f.name("uniq");
        let first = f.start(&name);
        let snapshot = f.m.events(&f.w, &name);
        let again = f.start(&name);
        assert_eq!(first.id, again.id);
        assert_eq!(f.rows(&name).len(), 1);
        f.list();
        f.close(&name);
        let after = f.m.events(&f.w, &name);
        assert!(after.len() > snapshot.len());
        assert_eq!(after[..snapshot.len()], snapshot[..]);
    }
);

parity!(
    simultaneous_starts_of_one_name_yield_one_task_and_one_box,
    covers = ["A-105"],
    tags = [],
    {
        let f = fx();
        f.w.prepend_rules("crabbox", [Rule::on(&["warmup"]).sleep_ms(1500)]);
        let name = f.name("race");
        let opts = StartOpts::default();
        let a = f.m.start_detached(&f.w, &f.repo.path, &name, &opts);
        let b = f.m.start_detached(&f.w, &f.repo.path, &name, &opts);
        let outs = [a.wait_with_output().unwrap(), b.wait_with_output().unwrap()];
        assert!(outs.iter().any(|o| o.status.success()), "no start won");
        assert_eq!(f.rows(&name).len(), 1);
        assert_eq!(f.n("crabbox", "warmup"), 1);
    }
);

parity!(
    bound_worktree_config_drives_setup,
    covers = ["REQ-29"],
    tags = ["new_only(WorktreeConfig)"],
    {
        let f = fx();
        let wt = bound_worktree(&f, "feature/cfg");
        write_json(
            &wt.join(".moor.yaml"),
            json!({ "setup": ["echo wt-config-marker"] }),
        );
        f.start_with(&f.name("cfg"), &bind_opts(&wt));
        assert_eq!(run_with(&f, "wt-config-marker").len(), 1);
    }
);
