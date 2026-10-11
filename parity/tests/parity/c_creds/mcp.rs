use parity::adapter::{ExitClass, StartOpts};
use parity::assert::assert_secret_absent;
use parity::parity;
use parity::world::World;
use serde_json::{json, Value};

use super::support::*;
use crate::moor;

fn mcp_user_config(f: &Fx, secret: &str) {
    let file = f.secret_file("mcp", secret);
    f.user(json!({
        "credentials": { "T": { "file": file } },
        "mcp_servers": {
            "srv": { "command": "npx", "args": ["--tok=${T}"], "env": { "K": "${T}" } },
            "web": { "url": "http://h.example.com/${T}", "headers": { "X": "${T}" } },
        },
    }));
}

fn mcp_arg(w: &World) -> Option<String> {
    w.calls("claude")
        .iter()
        .rev()
        .find_map(|c| c.flag("--mcp-config"))
}

parity!(
    box_agent_launch_delivers_mcp_config_without_leaking_secrets,
    covers = ["C-132", "C-133", "C-134", "C-136"],
    tags = ["new_only(BoxMcp)"],
    {
        let f = box_fx();
        let secret = f.secret("mcp");
        mcp_user_config(&f, &secret);
        write_file(
            &f.repo.path.join(".mcp.json"),
            &json!({ "mcpServers": { "own": {} } }).to_string(),
            0o644,
        );
        write_file(
            &f.repo.path.join(".claude/settings.local.json"),
            &json!({ "enableAllProjectMcpServers": true }).to_string(),
            0o644,
        );
        let before = repo_snapshot(&f.repo.path);
        let name = f.w.task_name("mcp");
        started_ok(&f, &name, Some("go"));
        let seen = run_box_agent(&f.w);
        assert!(
            mcp_arg(&f.w).is_some(),
            "Box agent claude invoked without an MCP config"
        );
        let text = seen.expect("MCP config readable from the Box agent's cwd");
        let doc: Value = serde_json::from_str(&text).expect("MCP config is JSON");
        for server in ["srv", "web"] {
            assert!(doc["mcpServers"].get(server).is_some(), "{server}: {text}");
        }
        assert!(!text.contains(&secret), "MCP config holds the secret value");
        assert_secret_absent(&f.w, &secret, &[]);
        assert_eq!(repo_snapshot(&f.repo.path), before, "repo untouched");
        finish(&f, &name);
        assert_secret_absent(&f.w, &secret, &[]);
    }
);

parity!(
    box_agent_gets_mcp_config_only_when_servers_configured,
    covers = ["C-134", "C-135", "C-136"],
    tags = ["new_only(BoxMcp)"],
    {
        let f = box_fx();
        let first = f.w.task_name("nomcp");
        started_ok(&f, &first, Some("go"));
        let seen = run_box_agent(&f.w);
        assert_eq!(mcp_arg(&f.w), None, "no servers, no MCP config");
        assert!(seen.is_none());
        finish(&f, &first);

        let secret = f.secret("mcp2");
        mcp_user_config(&f, &secret);
        let second = f.w.task_name("withmcp");
        started_ok(&f, &second, Some("go"));
        let seen = run_box_agent(&f.w);
        assert!(
            mcp_arg(&f.w).is_some(),
            "servers configured but Box agent got no MCP config"
        );
        assert!(seen.is_some_and(|t| t.contains("srv")));
        finish(&f, &second);

        f.user(json!({
            "credentials": { "T": { "file": f.w.root().join("secrets/missing") } },
            "mcp_servers": { "srv": { "command": "npx", "args": ["${T}"] } },
        }));
        let name = f.w.task_name("badmcp");
        assert_rejected_start(&f, &start_task(&f, &name, Some("go")), &name);
        assert_secret_absent(&f.w, &secret, &[]);
    }
);

parity!(
    worktree_moor_yaml_wins_for_box_size_else_main,
    covers = ["C-09"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        f.repo_cfg(json!({ "box": { "size": "large" } }));
        let wt = f.w.root().join("wt-explicit");
        let status =
            f.w.command("git")
                .args(["worktree", "add", "-q", "-b", "wt-branch"])
                .arg(&wt)
                .current_dir(&f.repo.path)
                .status()
                .expect("git worktree add");
        assert!(status.success());
        bind_listing(&f.w, &wt, "wt-branch");
        let class_of = |name: &str| {
            let opts = StartOpts {
                worktree: Some(wt.clone()),
                ..Default::default()
            };
            let o = moor().start(&f.w, &f.repo.path, name, &opts);
            assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
            let calls = f.w.calls("crabbox");
            let warm = calls
                .iter()
                .rfind(|c| c.argv[0] == "warmup")
                .expect("warmup");
            warm.flag("--class")
        };
        let fallback = f.w.task_name("wtmain");
        let main_class = class_of(&fallback);
        assert!(main_class.is_some());
        finish(&f, &fallback);
        write_file(
            &wt.join(".moor.yaml"),
            &json!({ "box": { "size": "medium" } }).to_string(),
            0o644,
        );
        let own = f.w.task_name("wtown");
        let own_class = class_of(&own);
        assert!(own_class.is_some());
        assert_ne!(own_class, main_class, "worktree size must win");
        finish(&f, &own);
    }
);

parity!(
    bound_worktree_config_drives_setup_and_box_credentials,
    covers = [],
    tags = ["new_only(WorktreeRepoConfig)"],
    {
        let f = fx();
        flow_fakes(&f.w);
        let (tok, keyf) = (f.secret("tok"), f.secret("keyf"));
        creds_config(&f, &tok, &keyf);
        f.repo_cfg(json!({ "setup": ["true"] }));
        let wt = f.w.root().join("wt-cfg");
        let status =
            f.w.command("git")
                .args(["worktree", "add", "-q", "-b", "wt-cfg-branch"])
                .arg(&wt)
                .current_dir(&f.repo.path)
                .status()
                .expect("git worktree add");
        assert!(status.success());
        bind_listing(&f.w, &wt, "wt-cfg-branch");
        write_file(
            &wt.join(".moor.yaml"),
            &json!({ "setup": [SETUP_PROBE], "box_credentials": pair_credentials() }).to_string(),
            0o644,
        );
        let name = f.w.task_name("wtcfg");
        let opts = StartOpts {
            worktree: Some(wt.clone()),
            ..Default::default()
        };
        let o = moor().start(&f.w, &f.repo.path, &name, &opts);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let task = o.ok().clone();
        assert_eq!(probe_file(&task, &f, "got-env"), tok.len().to_string());
        finish(&f, &name);
    }
);
