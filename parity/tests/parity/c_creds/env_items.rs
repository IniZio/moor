use std::os::unix::fs::PermissionsExt;

use parity::adapter::{EnvOpts, ExitClass};
use parity::assert::assert_secret_absent;
use parity::parity;
use serde_json::json;

use super::env_shape::{env, run};
use super::support::*;

parity!(
    env_never_prints_credential_material,
    covers = ["C-06", "C-38"],
    tags = [],
    {
        let f = fx();
        let (sa, sb, sc) = (f.secret("a"), f.secret("b"), f.secret("c"));
        let marker = f.secret("argmarker");
        let a_file = f.secret_file("a", &sa);
        write_file(&f.w.home().join("secrets/b"), &sb, 0o600);
        f.user(json!({
        "credentials": {
            "A": { "cmd": format!("FOO=1 BAR=2 sh -c 'cat \"$0\"' {a_file} --marker={marker}") },
            "B": { "file": "~/secrets/b" },
            "C": { "env": "PARITY_V_C" },
        },
        "mcp_servers": {
            "m": { "command": "npx", "args": ["--tok=${A}"], "env": { "K": "${B}" } },
            "u": { "url": "http://h/?k=${C}", "headers": { "X": "${C}" } },
        },
    }));
        let opts = |text| EnvOpts {
            text,
            set_env: vec![("PARITY_V_C".into(), sc.clone())],
            ..Default::default()
        };
        let json_run = run(&f, opts(false));
        let text_run = run(&f, opts(true));
        let r = json_run.ok();
        assert_eq!(text_run.class, ExitClass::Ok, "{}", text_run.stderr);
        for (name, resolves) in [("A", true), ("B", true), ("C", true)] {
            assert_eq!(
                r.credential(name).and_then(|c| c.resolves),
                Some(resolves),
                "{name}"
            );
        }
        let detail = |n: &str| {
            r.credential(n)
                .and_then(|c| c.detail.clone())
                .unwrap_or_default()
        };
        assert!(
            detail("A").contains("sh") && !detail("A").contains("FOO"),
            "{}",
            detail("A")
        );
        assert!(!detail("A").contains(&a_file) && !detail("A").contains("marker"));
        assert!(detail("B").contains("~/secrets/b"), "{}", detail("B"));
        assert!(detail("C").contains("PARITY_V_C"), "{}", detail("C"));
        for out in [&json_run.stdout, &text_run.stdout] {
            for hidden in [&sa, &sb, &sc, &marker] {
                assert!(
                    !out.contains(hidden.as_str()),
                    "env output carries {hidden}"
                );
            }
        }
        for s in [&sa, &sb, &sc, &marker] {
            assert_secret_absent(&f.w, s, &[]);
        }
    }
);

parity!(env_github_status_block, covers = ["C-36"], tags = [], {
    let f = fx();
    let status = |f: &Fx| json_of(&env(f))["github"].clone();
    let none = status(&f);
    assert_eq!(none["identity"], "auto");
    assert!(none["app_id"].is_null());
    assert_eq!(none["mint"]["status"], "skipped");
    let auto_login = none["login"].clone();
    f.user(json!({ "github": { "identity": "bot" } }));
    let bot = status(&f);
    assert_eq!(bot["login"], "n/a");
    assert_ne!(bot["login"], auto_login);

    let store = f.w.state_dir().join("github");
    let (client_secret, refresh) = (f.secret("clientsecret"), f.secret("refresh"));
    write_file(
        &store.join("app.json"),
        &json!({ "app_id": "4242", "client_id": "Iv1.x", "client_secret": client_secret })
            .to_string(),
        0o600,
    );
    write_file(&store.join("app.pem"), "not a pem\n", 0o600);
    write_file(
        &store.join("user.json"),
        &json!({ "refresh_token": refresh }).to_string(),
        0o600,
    );
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    let key = format!("url.{}.insteadOf", f.repo.bare.display());
    let unset =
        f.w.command("git")
            .args(["config", "--global", "--unset", &key])
            .status();
    assert!(unset.expect("git config").success());
    f.user(json!({ "github": { "identity": "you" } }));
    let stored = env(&f);
    assert_eq!(stored.class, ExitClass::Ok, "{}", stored.stderr);
    let g = json_of(&stored)["github"].clone();
    assert_eq!(g["identity"], "you");
    assert_eq!(g["app_id"], "4242");
    assert!(
        g["login"] != "n/a" && g["login"] != auto_login,
        "{}",
        g["login"]
    );
    assert_eq!(g["mint"]["status"], "failed", "{g}");
    for s in [&client_secret, &refresh] {
        assert_secret_absent(&f.w, s, &["github/"]);
    }
});

parity!(env_user_mcp_items, covers = ["C-39"], tags = [], {
    let f = fx();
    let secret = f.secret("mcp");
    let file = f.secret_file("t", &secret);
    f.user(json!({
        "credentials": { "T": { "file": file }, "DEAD": { "file": "/nonexistent/x" } },
        "mcp_servers": {
            "cmdsrv": { "command": "npx", "args": ["--tok=${T}"] },
            "urlsrv": { "url": "http://h.example.com/mcp", "headers": { "A": "${T}" } },
            "broken": { "command": "x", "args": ["${DEAD}"] },
        },
    }));
    let o = env(&f);
    let r = o.ok();
    let item = |n: &str| r.item("mcp", n).unwrap_or_else(|| panic!("{n} missing"));
    assert_eq!(item("cmdsrv").source, "user");
    assert_eq!(item("cmdsrv").detail.as_deref(), Some("npx"));
    assert_eq!(item("cmdsrv").resolves, Some(true));
    assert_eq!(
        item("urlsrv").detail.as_deref(),
        Some("http://h.example.com/mcp")
    );
    assert_eq!(item("broken").resolves, Some(false));
    assert!(item("broken")
        .error
        .as_deref()
        .is_some_and(|e| e.contains("DEAD")));
    assert_secret_absent(&f.w, &secret, &[]);
});

parity!(
    env_repo_mcp_items_follow_claude_approval,
    covers = ["C-40"],
    tags = [],
    {
        let f = fx();
        let repo = f.repo.path.clone();
        let servers = json!({ "mcpServers": { "a": {}, "b": {}, "c": {} } });
        assert!(
            env(&f).ok().of_kind("mcp").is_empty(),
            "no .mcp.json, no items"
        );
        write_file(
            &repo.join(".mcp.json"),
            &json!({ "other": 1 }).to_string(),
            0o644,
        );
        assert!(
            env(&f).ok().of_kind("mcp").is_empty(),
            "no mcpServers, no items"
        );

        write_file(&repo.join(".mcp.json"), &servers.to_string(), 0o644);
        write_file(
            &repo.join(".claude/settings.json"),
            &json!({ "enabledMcpjsonServers": ["a"] }).to_string(),
            0o644,
        );
        write_file(
            &repo.join(".claude/settings.local.json"),
            &json!({ "disabledMcpjsonServers": ["b"] }).to_string(),
            0o644,
        );
        let o = env(&f);
        let r = o.ok();
        let state = |n: &str| {
            r.item("mcp", n)
                .map(|i| (i.source.as_str(), i.resolves, i.error.is_some()))
        };
        assert_eq!(state("a"), Some(("repo", Some(true), false)));
        assert_eq!(state("b"), Some(("repo", Some(false), true)));
        assert_eq!(state("c"), Some(("repo", Some(false), true)));
        assert_ne!(
            r.item("mcp", "b").unwrap().error,
            r.item("mcp", "c").unwrap().error
        );

        write_file(
            &repo.join(".claude/settings.local.json"),
            &json!({ "enableAllProjectMcpServers": true }).to_string(),
            0o644,
        );
        let all = env(&f);
        assert_eq!(
            all.ok().item("mcp", "c").and_then(|i| i.resolves),
            Some(true)
        );

        write_file(&repo.join(".mcp.json"), "{ not json", 0o644);
        let bad = env(&f);
        let items = bad.ok().of_kind("mcp");
        assert_eq!(items.len(), 1, "invalid JSON is one failed item");
        assert_eq!(items[0].resolves, Some(false));
        assert!(items[0].error.is_some());
    }
);

parity!(
    env_egress_text_never_counts_zero_entries,
    covers = [],
    tags = ["new_only(EnvEgressText)"],
    {
        let f = fx();
        f.user(json!({ "box": { "provider": "local-container" } }));
        let o = env(&f);
        let items = o.ok().of_kind("egress");
        assert_eq!(items.len(), 1);
        let detail = items[0].detail.clone().unwrap_or_default();
        assert!(
            !detail.contains(" 0 "),
            "egress row counts zero entries: {detail}"
        );
    }
);

parity!(
    repo_credentials_gh_token_ref_is_a_hard_error,
    covers = [],
    tags = ["new_only(RepoGhTokenRef)"],
    {
        let f = fx();
        f.repo_cfg(json!({ "credentials": ["GH_TOKEN"] }));
        assert_rejected(&env(&f), "repo credentials GH_TOKEN, box");
    }
);
