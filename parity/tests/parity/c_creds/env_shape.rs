use parity::adapter::{EnvOpts, EnvReport, ExitClass, Outcome};
use parity::assert::assert_secret_absent;
use parity::parity;
use serde_json::{json, Value};

use super::support::*;
use crate::moor;

pub(super) fn run(f: &Fx, opts: EnvOpts) -> Outcome<EnvReport> {
    f.env_with(&opts)
}

pub(super) fn env(f: &Fx) -> Outcome<EnvReport> {
    run(f, EnvOpts::default())
}

parity!(env_exit_classes, covers = ["C-24"], tags = [], {
    let f = fx();
    f.user(json!({ "credentials": { "GONE": { "file": "/nonexistent/path" } } }));
    let unresolved = env(&f);
    let report = unresolved.ok();
    assert_eq!(
        report.credential("GONE").and_then(|c| c.resolves),
        Some(false)
    );

    let outside = moor().env(
        &f.w,
        &plain_dir(&f),
        &EnvOpts {
            set_env: vec![(
                "GIT_CEILING_DIRECTORIES".into(),
                f.w.root().to_string_lossy().into_owned(),
            )],
            ..Default::default()
        },
    );
    assert_rejected(&outside, "outside a git repo");
});

parity!(env_json_shape, covers = ["C-25"], tags = [], {
    let f = fx();
    let file = f.secret_file("c", "v");
    f.user(json!({ "credentials": { "C": { "file": file } } }));
    f.repo_cfg(json!({ "agent": { "model": "haiku" }, "egress": ["a.example.com"] }));
    let o = env(&f);
    o.ok();
    let v = json_of(&o);
    for key in ["repo", "items", "agent", "github"] {
        assert!(v.get(key).is_some(), "missing top-level {key}");
    }
    assert!(v["agent"].get("model").is_some());
    for key in ["identity", "app_id", "login", "mint", "resolved"] {
        assert!(v["github"].get(key).is_some(), "missing github.{key}");
    }
    assert!(v["github"]["mint"]["status"].is_string());
    for key in ["mode", "app_installed", "token_source", "error"] {
        assert!(
            v["github"]["resolved"].get(key).is_some(),
            "missing resolved.{key}"
        );
    }
    let kinds = [
        "identity",
        "credential",
        "tool",
        "setup",
        "mcp",
        "provision",
        "egress",
        "run",
        "box",
        "dotfiles",
    ];
    for item in v["items"].as_array().expect("items array") {
        let kind = item["kind"].as_str().expect("kind");
        assert!(kinds.contains(&kind), "unknown kind {kind}");
        assert!(["user", "repo", "default"].contains(&item["source"].as_str().expect("source")));
        assert!(item["name"].is_string());
    }
});

parity!(
    env_text_output_marks_resolution,
    covers = ["C-26"],
    tags = [],
    {
        let f = fx();
        let file = f.secret_file("c", "v");
        f.user(json!({ "credentials": {
        "GOOD": { "file": file }, "BAD": { "file": "/nonexistent/path" }
    } }));
        let o = run(
            &f,
            EnvOpts {
                text: true,
                ..Default::default()
            },
        );
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(
            serde_json::from_str::<Value>(&o.stdout).is_err(),
            "text mode printed JSON"
        );
        let lines: Vec<&str> = o.stdout.lines().collect();
        assert!(lines[0].starts_with("agent"), "first line: {}", lines[0]);
        let good = lines
            .iter()
            .find(|l| l.contains("GOOD"))
            .expect("GOOD line");
        let bad = lines.iter().find(|l| l.contains("BAD")).expect("BAD line");
        assert!(
            good.contains('\u{2713}') && !good.contains('\u{2717}'),
            "{good}"
        );
        assert!(
            bad.contains('\u{2717}') && !bad.contains('\u{2713}'),
            "{bad}"
        );
    }
);

parity!(env_item_order_is_fixed, covers = ["C-27"], tags = [], {
    let f = fx();
    let dots = f.w.root().join("dots");
    std::fs::create_dir_all(&dots).expect("mkdir");
    let file = f.secret_file("c", "v");
    f.user(json!({
        "git": { "name": "N", "email": "n@example.com" },
        "credentials": { "ZED": { "file": file }, "ALPHA": { "file": file }, "MID": { "file": file } },
        "mcp_servers": { "usrmcp": { "command": "npx" } },
        "box": { "provider": "sprites" },
        "dotfiles": { "dir": dots.to_string_lossy() },
    }));
    write_file(&f.repo.path.join(".env"), "A=1\n", 0o644);
    write_file(
        &f.repo.path.join(".mcp.json"),
        &json!({ "mcpServers": { "repomcp": {} } }).to_string(),
        0o644,
    );
    f.repo_cfg(json!({
        "tools": ["jq"], "setup": ["true"], "provision": [".env"],
        "run": { "env": { "CI": "1" }, "path": ["/opt/bin"] },
        "egress": ["a.example.com"], "credentials": ["ALPHA"], "box_credentials": ["MID"],
        "box": { "size": "medium" },
    }));
    let o = env(&f);
    let items = &o.ok().items;
    let rank = |k: &str| {
        [
            "identity",
            "credential",
            "tool",
            "setup",
            "mcp",
            "provision",
            "run",
            "egress",
            "box",
            "dotfiles",
        ]
        .iter()
        .position(|x| *x == k)
        .unwrap_or_else(|| panic!("unexpected kind {k}"))
    };
    let ranks: Vec<usize> = items.iter().map(|i| rank(&i.kind)).collect();
    assert!(
        ranks.windows(2).all(|p| p[0] <= p[1]),
        "kinds out of order: {ranks:?}"
    );
    for k in [
        "identity",
        "credential",
        "tool",
        "setup",
        "mcp",
        "provision",
        "run",
        "egress",
        "box",
        "dotfiles",
    ] {
        assert!(items.iter().any(|i| i.kind == k), "no {k} item");
    }
    let creds: Vec<&str> = o
        .ok()
        .of_kind("credential")
        .iter()
        .map(|i| i.name.as_str())
        .collect();
    assert_eq!(creds, ["ALPHA", "MID", "ZED"]);
    let mcp: Vec<&str> = o
        .ok()
        .of_kind("mcp")
        .iter()
        .map(|i| i.source.as_str())
        .collect();
    assert_eq!(mcp, ["user", "repo"]);
    let boxed: Vec<&str> = o
        .ok()
        .of_kind("box")
        .iter()
        .map(|i| i.name.as_str())
        .collect();
    assert_eq!(boxed, ["provider", "size", "gh"]);
    let run: Vec<&str> = o
        .ok()
        .of_kind("run")
        .iter()
        .map(|i| i.name.as_str())
        .collect();
    assert_eq!(run.len(), 2, "{run:?}");
});

parity!(
    env_credential_sources_and_union,
    covers = ["C-28"],
    tags = [],
    {
        let f = fx();
        let file = f.secret_file("c", "v");
        f.user(json!({ "credentials": {
        "USERONLY": { "file": file }, "UNUSED": { "file": file }
    } }));
        f.repo_cfg(json!({
            "credentials": ["USERONLY", "REPOREF"], "box_credentials": ["BOXONLY"],
        }));
        let o = env(&f);
        let r = o.ok();
        let user_only = r.credential("USERONLY").expect("USERONLY");
        assert_eq!(
            (user_only.source.as_str(), user_only.resolves),
            ("user", Some(true))
        );
        assert!(
            r.credential("UNUSED").is_some(),
            "every user credential is listed"
        );
        for name in ["REPOREF", "BOXONLY"] {
            let c = r
                .credential(name)
                .unwrap_or_else(|| panic!("{name} missing"));
            assert_eq!(
                (c.source.as_str(), c.resolves),
                ("repo", Some(false)),
                "{name}"
            );
            assert!(c.error.is_some(), "{name} has no error");
        }
    }
);

parity!(env_delivery_labels, covers = ["C-29"], tags = [], {
    let f = fx();
    let file = f.secret_file("c", "v");
    f.user(json!({ "credentials": {
        "LISTED": { "file": file }, "PLAIN": { "file": file },
        "CLAUDE_CODE_OAUTH_TOKEN": { "file": file },
    } }));
    f.repo_cfg(json!({ "box_credentials": ["LISTED"] }));
    let o = env(&f);
    let r = o.ok();
    let label = |n: &str| r.credential(n).and_then(|c| c.delivery.clone());
    assert_eq!(label("LISTED").as_deref(), Some("stdin/file"));
    assert_eq!(label("PLAIN").as_deref(), Some("host only"));
    assert_eq!(label("CLAUDE_CODE_OAUTH_TOKEN").as_deref(), Some("proxy"));
    assert!(r
        .items
        .iter()
        .all(|i| i.delivery.as_deref() != Some("injected")));
});

parity!(env_always_lists_box_gh_row, covers = ["C-31"], tags = [], {
    let f = fx();
    f.user(json!({ "box": { "provider": "sprites" } }));
    f.repo_cfg(json!({ "egress": ["a.example.com"] }));
    let r = env(&f);
    let gh = r.ok().item("box", "gh").expect("box gh row");
    assert_eq!(gh.delivery.as_deref(), Some("proxy"));
    assert!(r.ok().items.iter().all(|i| i.name != "GH_TOKEN"));
    assert!(f.w.all_calls().is_empty(), "env must not call any tool");
});

parity!(env_egress_item_variants, covers = ["C-32"], tags = [], {
    let f = fx();
    let egress_of = |o: &Outcome<EnvReport>| {
        let r = o.ok();
        let items = r.of_kind("egress");
        assert_eq!(items.len(), 1, "exactly one egress row");
        (
            items[0].name.clone(),
            items[0].detail.clone().unwrap_or_default(),
        )
    };
    f.user(json!({ "box": { "provider": "sprites" } }));
    f.repo_cfg(json!({ "egress": ["a.example.com"] }));
    let (name, detail) = egress_of(&env(&f));
    assert_eq!(name, "allow");
    assert!(detail.contains("a.example.com"), "{detail}");

    f.repo_cfg(json!({}));
    let (open_name, open_detail) = egress_of(&env(&f));
    assert_eq!(open_name, "allow");
    assert!(open_detail.contains("github.com"), "{open_detail}");

    f.user(json!({ "box": { "provider": "local-container" } }));
    let (none_name, none_detail) = egress_of(&env(&f));
    assert_eq!(none_name, "policy");
    assert_ne!(
        open_detail, none_detail,
        "proxy-active allow vs unsupported must read differently"
    );

    f.repo_cfg(json!({ "egress": ["a.example.com", "b.example.com"] }));
    let (n_name, n_detail) = egress_of(&env(&f));
    assert_eq!(n_name, "policy");
    assert_ne!(
        n_detail, none_detail,
        "unsupported with entries must say so"
    );
    assert!(n_detail.contains('2'), "{n_detail}");
});

parity!(
    env_enforce_note_on_namespace,
    covers = ["C-33"],
    tags = [],
    {
        let f = fx();
        f.user(json!({ "box": { "provider": "namespace-devbox" } }));
        let o = env(&f);
        assert!(o.ok().item("egress", "enforce").is_some(), "note missing");
        f.user(json!({ "box": { "provider": "namespace-devbox", "enforce_egress": true } }));
        assert!(env(&f).ok().item("egress", "enforce").is_none());
        f.user(json!({ "box": { "provider": "sprites" } }));
        assert!(env(&f).ok().item("egress", "enforce").is_none());
    }
);

parity!(
    env_proxy_domains_rejected_only_with_policy_provider,
    covers = ["C-34"],
    tags = [],
    {
        let f = fx();
        f.repo_cfg(json!({ "egress": ["api.github.com"] }));
        f.user(json!({ "box": { "provider": "sprites" } }));
        assert_rejected(&env(&f), "sprites + box + api.github.com");
        f.repo_cfg(json!({ "egress": ["api.anthropic.com"] }));
        assert_rejected(&env(&f), "sprites + box + api.anthropic.com");
        f.user(json!({ "box": { "provider": "local-container" } }));
        assert_accepted(&env(&f), "local-container never checks");
    }
);

parity!(
    env_credential_source_semantics,
    covers = ["C-05"],
    tags = [],
    {
        let f = fx();
        let secret = f.secret("src");
        let good = f.secret_file("good", &format!("{secret}\r\n"));
        let blank = f.secret_file("blank", "  \n");
        let empty = f.secret_file("empty", "");
        write_file(&f.w.home().join("tilde/secret"), &secret, 0o600);
        let dir = f.w.root().join("secrets/adir");
        std::fs::create_dir_all(&dir).expect("mkdir");
        f.user(json!({ "credentials": {
        "CMD_OK": { "cmd": format!("cat {good}") },
        "CMD_FAIL": { "cmd": format!("cat {good}; exit 3") },
        "CMD_SILENT": { "cmd": "true" },
        "CMD_BLANK": { "cmd": format!("cat {blank}") },
        "CMD_STDIN": { "cmd": "cat" },
        "FILE_OK": { "file": good },
        "FILE_TILDE": { "file": "~/tilde/secret" },
        "FILE_MISSING": { "file": "/nonexistent/secret" },
        "FILE_EMPTY": { "file": empty },
        "FILE_DIR": { "file": dir.to_string_lossy() },
        "ENV_OK": { "env": "PARITY_V_SET" },
        "ENV_UNSET": { "env": "PARITY_V_UNSET" },
        "ENV_EMPTY": { "env": "PARITY_V_EMPTY" },
    } }));
        let o = run(
            &f,
            EnvOpts {
                set_env: vec![
                    ("PARITY_V_SET".into(), secret.clone()),
                    ("PARITY_V_EMPTY".into(), String::new()),
                ],
                ..Default::default()
            },
        );
        let r = o.ok();
        let want = [
            ("CMD_OK", true),
            ("CMD_FAIL", false),
            ("CMD_SILENT", false),
            ("CMD_BLANK", false),
            ("CMD_STDIN", false),
            ("FILE_OK", true),
            ("FILE_TILDE", true),
            ("FILE_MISSING", false),
            ("FILE_EMPTY", false),
            ("FILE_DIR", false),
            ("ENV_OK", true),
            ("ENV_UNSET", false),
            ("ENV_EMPTY", false),
        ];
        for (name, resolves) in want {
            let c = r
                .credential(name)
                .unwrap_or_else(|| panic!("{name} missing"));
            assert_eq!(c.resolves, Some(resolves), "{name}: {:?}", c.error);
            assert_eq!(c.error.is_some(), !resolves, "{name} error presence");
        }
        assert_secret_absent(&f.w, &secret, &[]);
    }
);
