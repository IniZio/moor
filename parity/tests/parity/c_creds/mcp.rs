use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use parity::adapter::{ExitClass, StartOpts};
use parity::assert::assert_secret_absent;
use parity::fake::Rule;
use parity::parity;
use serde_json::{json, Value};

use super::support::*;
use crate::moor;

parity!(
    mcp_config_file_for_dispatched_host_agent,
    covers = ["C-132", "C-133", "C-134"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let secret = f.secret("mcp");
        let file = f.secret_file("mcp", &secret);
        f.user(json!({
            "credentials": { "T": { "file": file } },
            "mcp_servers": {
                "srv": { "command": "npx", "args": ["--tok=${T}"], "env": { "K": "${T}" } },
                "web": { "url": "http://h.example.com/${T}", "headers": { "X": "${T}" } },
            },
        }));
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
        let path = PathBuf::from(
            bg_call(&f.w)
                .flag("--mcp-config")
                .expect("--mcp-config flag"),
        );
        assert!(
            path.starts_with(f.w.state_dir().join("mcp")),
            "{}",
            path.display()
        );
        let mode = std::fs::metadata(&path)
            .expect("mcp file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let doc: Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
        let servers = &doc["mcpServers"];
        assert_eq!(servers["srv"]["args"][0], format!("--tok={secret}"));
        assert_eq!(servers["srv"]["env"]["K"], secret.as_str());
        assert_eq!(
            servers["web"]["url"],
            format!("http://h.example.com/{secret}")
        );
        assert_eq!(servers["web"]["headers"]["X"], secret.as_str());
        assert!(servers["srv"].get("url").is_none() && servers["web"].get("command").is_none());
        assert_secret_absent(&f.w, &secret, &["mcp/"]);
        assert_eq!(
            repo_snapshot(&f.repo.path),
            before,
            "moor writes nothing into the repo"
        );
        finish(&f, &name);
        assert!(mcp_files(&f.w).is_empty(), "mcp file survives close");
        assert_secret_absent(&f.w, &secret, &[]);
    }
);

parity!(
    mcp_config_absent_without_servers_and_removed_on_failed_dispatch,
    covers = ["C-134"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let name = f.w.task_name("nomcp");
        started_ok(&f, &name, Some("go"));
        assert_eq!(bg_call(&f.w).flag("--mcp-config"), None);
        assert!(mcp_files(&f.w).is_empty());
        finish(&f, &name);

        let f = fx();
        flow_fakes(&f.w);
        let secret = f.secret("mcpfail");
        let file = f.secret_file("mcp", &secret);
        f.user(json!({
            "credentials": { "T": { "file": file } },
            "mcp_servers": { "srv": { "command": "npx", "args": ["${T}"] } },
        }));
        f.w.prepend_rules(
            "claude",
            [Rule::any()
                .contains("--bg")
                .exec("cp '{flag:--mcp-config}' '{root}/mcp-seen'")
                .exit(1)
                .stderr("dispatch refused\n")],
        );
        let name = f.w.task_name("failmcp");
        let _ = start_task(&f, &name, Some("go"), None);
        let seen = std::fs::read_to_string(f.w.root().join("mcp-seen"))
            .expect("mcp config existed at dispatch time");
        assert!(seen.contains(&secret), "mcp config lacked the server args");
        assert!(
            mcp_files(&f.w).is_empty(),
            "failed dispatch leaves {:?}",
            mcp_files(&f.w)
        );
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
