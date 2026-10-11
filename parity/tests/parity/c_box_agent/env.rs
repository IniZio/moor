use super::*;

fn origin(b: &Bx, url: Option<&str>) {
    let git = |args: &[&str]| {
        let st =
            b.w.command("git")
                .current_dir(&b.repo.path)
                .args(args)
                .status()
                .expect("git");
        assert!(st.success(), "git {args:?}");
    };
    match url {
        Some(u) => git(&["remote", "set-url", "origin", u]),
        None => git(&["remote", "remove", "origin"]),
    }
}

fn leaves_git_config_or_credential_on_box(w: &World) -> bool {
    w.box_root().join("home/.gitconfig").exists()
        || files_under(&w.box_root()).iter().any(|f| {
            std::fs::read_to_string(f)
                .is_ok_and(|t| t.contains("credential") || t.contains("x-access-token"))
        })
}

parity!(
    box_agent_git_env_routes_github_through_proxy_and_exports_push_url,
    covers = ["C-101", "REQ-34"],
    tags = [],
    {
        let b = bx(true);
        let bare = b.repo.bare.display().to_string();
        let gh = "https://github.com/acme/widget";
        let cases: [(&str, Option<&str>, Option<String>); 5] = [
            ("https", Some(gh), Some(gh.into())),
            (
                "sshurl",
                Some("ssh://git@github.com/acme/widget.git"),
                Some(format!("{gh}.git")),
            ),
            (
                "scp",
                Some("git@github.com:acme/widget.git"),
                Some(format!("{gh}.git")),
            ),
            ("local", Some(&bare), Some(bare.clone())),
            ("none", None, None),
        ];
        for (tag, url, want_push) in cases {
            origin(&b, url);
            let name = b.w.task_name(tag);
            let l = launched(&b, &name, "push env");
            let r = ran(&b);
            let e = &r.env;
            assert_eq!(
                e.get("GIT_CONFIG_COUNT").map(String::as_str),
                Some("1"),
                "{tag}"
            );
            let key = format!(
                "url.http://127.0.0.1:{}/t/{}/github/.insteadOf",
                l.box_port, l.nonce
            );
            assert_eq!(e.get("GIT_CONFIG_KEY_0"), Some(&key), "{tag}");
            assert_eq!(
                e.get("GIT_CONFIG_VALUE_0").map(String::as_str),
                Some("https://github.com/"),
                "{tag}"
            );
            assert_eq!(e.get("MOOR_PUSH_URL"), want_push.as_ref(), "{tag}");
            assert!(
                !e.contains_key("GIT_CONFIG_KEY_1"),
                "{tag}: no credential helper"
            );
            assert!(
                !leaves_git_config_or_credential_on_box(&b.w),
                "{tag}: git auth left a file on the Box"
            );
            finish(&b, &name);
        }
    }
);

parity!(
    box_agent_gh_env_puts_shim_first_and_names_canonical_repo_only_when_different,
    covers = ["C-102"],
    tags = [],
    {
        let b = bx(true);
        let repo_path = b.fx.repo_path();
        for (tag, name_in_api, canonical) in [
            ("same", "widget", None),
            ("case", "WIDGET", None),
            ("moved", "Gadget", Some("acme/Gadget")),
        ] {
            let mut repo = b.fx.repo_json();
            repo["name"] = json!(name_in_api);
            b.gh.reroute("GET", &repo_path, vec![Reply::ok(&repo)]);
            let name = b.w.task_name(tag);
            let l = launched(&b, &name, "gh env");
            let r = ran(&b);
            let e = &r.env;
            let api = format!("http://127.0.0.1:{}/t/{}/github-api", l.box_port, l.nonce);
            assert_eq!(e.get("MOOR_GH_API"), Some(&api), "{tag}");
            assert_eq!(
                e.get("MOOR_GH_REPO").map(String::as_str),
                Some("acme/widget"),
                "{tag}"
            );
            assert_eq!(
                e.get("MOOR_GH_REPO_CANONICAL").map(String::as_str),
                canonical,
                "{tag}"
            );
            let first = e["PATH"].split(':').next().unwrap_or_default();
            assert!(
                first.ends_with("/bin") && first.contains("moor-cred-"),
                "{tag}: {first}"
            );
            assert!(
                !e.contains_key("GH_TOKEN") && !e.contains_key("GITHUB_TOKEN"),
                "{tag}"
            );
            assert!(
                r.find.contains("/bin/gh"),
                "{tag}: shim present while the agent runs"
            );
            finish(&b, &name);
        }
    }
);

parity!(
    box_agent_without_github_origin_gets_proxy_and_git_env_but_no_shim,
    covers = ["C-119", "C-101", "C-102"],
    tags = [],
    {
        let b = bx(false);
        let name = b.w.task_name("plain");
        let l = launched(&b, &name, "no github");
        assert_eq!(l.files, ["600 brief.md", "600 proxy-nonce", "700 ."]);
        assert!(l.shim.is_none());
        let r = ran(&b);
        for var in ["MOOR_GH_API", "MOOR_GH_REPO", "MOOR_GH_REPO_CANONICAL"] {
            assert!(!r.env.contains_key(var), "{var} without a GitHub origin");
        }
        assert!(
            !r.env["PATH"].contains("moor-cred-"),
            "no shim directory on PATH"
        );
        assert_eq!(
            r.env.get("ANTHROPIC_BASE_URL"),
            Some(&format!("http://127.0.0.1:{}", l.box_port))
        );
        assert_eq!(r.env.get("GIT_CONFIG_COUNT").map(String::as_str), Some("1"));
        assert!(r.env.contains_key("GIT_CONFIG_KEY_0"));
        for word in ["shim", "MOOR_GH", "`gh"] {
            assert!(!l.brief.contains(word), "brief mentions {word}");
        }
        assert!(
            b.gh.requests().is_empty(),
            "no mint for a non-GitHub origin"
        );
        finish(&b, &name);
    }
);

parity!(
    box_agent_applies_run_env_and_path_before_proxy_and_shim_path,
    covers = ["C-103"],
    tags = [],
    {
        let b = bx(true);
        repo_cfg(
            &b,
            json!({ "run": { "env": { "PARITY_RUN": "it's set" }, "path": ["/opt/parity-tools"] } }),
        );
        let name = b.w.task_name("prefix");
        launched(&b, &name, "run prefix");
        let r = ran(&b);
        assert_eq!(
            r.env.get("PARITY_RUN").map(String::as_str),
            Some("it's set")
        );
        let parts: Vec<&str> = r.env["PATH"].split(':').collect();
        let at = |needle: &str| {
            parts
                .iter()
                .position(|p| p.contains(needle))
                .unwrap_or_else(|| panic!("{needle} missing from PATH {parts:?}"))
        };
        assert!(at("moor-cred-") < at(".local/bin"));
        assert!(at(".local/bin") < at(".bun/bin"));
        assert!(at(".bun/bin") < at("/opt/parity-tools"));
        finish(&b, &name);
    }
);

parity!(
    box_agent_claude_runs_with_bypass_settings_and_brief_file,
    covers = ["C-104", "C-105", "REQ-39", "REQ-33"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("claude");
        let l = launched(&b, &name, "GOALTEXT-claude-argv");
        let r = ran(&b);
        let wd = workdir(&b.w);
        let call = agent_call(&b.w);
        assert_eq!(
            call.argv,
            [
                "--dangerously-skip-permissions".to_string(),
                "--settings".to_string(),
                format!("{}/.moor/agent-hooks.json", wd.display()),
                "Read and follow .moor/brief.md".to_string(),
            ]
        );
        assert_eq!(Path::new(&call.cwd), wd.as_path());
        let brief = wd.join(".moor/brief.md");
        assert_eq!(std::fs::read_to_string(&brief).unwrap_or_default(), l.brief);
        assert_eq!(mode_of(&brief), 0o600);
        let hooks: Value = serde_json::from_str(
            &std::fs::read_to_string(wd.join(".moor/agent-hooks.json")).expect("hooks file"),
        )
        .expect("hooks json");
        for (event, state) in [
            ("UserPromptSubmit", "working"),
            ("Stop", "idle"),
            ("Notification", "waiting_input"),
        ] {
            let cmd = hooks["hooks"][event][0]["hooks"][0]["command"]
                .as_str()
                .unwrap_or_default();
            assert!(
                cmd.contains("agent-state.json") && cmd.contains(state),
                "{event}: {cmd}"
            );
        }
        assert_eq!(r.state["state"], "starting", "state reset at launch");
        assert_eq!(r.env.get("CLAUDE_CODE_OAUTH_TOKEN"), Some(&l.nonce));
        assert!(r.env["ANTHROPIC_BASE_URL"].starts_with("http://127.0.0.1:"));
        assert_order(
            &b.w.all_calls(),
            &[
                ("crabbox", &["tunnel"]),
                ("claude", &["--dangerously-skip-permissions"]),
            ],
        );
        finish(&b, &name);
    }
);

parity!(
    box_agent_model_flag_only_when_given_and_host_agents_never_bypass,
    covers = ["C-104"],
    tags = [],
    {
        let b = bx(false);
        let mut seen: Vec<Option<String>> = Vec::new();
        let runs = [
            ("bare", json!({}), None),
            ("flag", json!({}), Some("flag-model")),
            (
                "repo",
                json!({ "agent": { "model": "repo-model" } }),
                Some("flag-model"),
            ),
        ];
        for (tag, cfg, flag) in runs {
            repo_cfg(&b, cfg);
            let name = b.w.task_name(tag);
            let opts = start_opts(Some("model"), None, flag);
            assert_eq!(start_with(&b, &name, &opts).class, ExitClass::Ok);
            ran(&b);
            seen.push(arg_after(&agent_call(&b.w), "--model"));
            finish(&b, &name);
        }
        assert_eq!(
            seen,
            [
                None,
                Some("flag-model".to_string()),
                Some("repo-model".to_string())
            ]
        );
        repo_cfg(&b, json!({ "agent": { "location": "host" } }));
        let host = b.w.task_name("host");
        let o = go(&b, &host, "host agent");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let bg: Vec<Call> =
            b.w.calls("claude")
                .into_iter()
                .filter(|c| c.argv.iter().any(|a| a == "--bg"))
                .collect();
        assert_eq!(bg.len(), 1, "host agent dispatched through claude --bg");
        assert!(!bg[0]
            .argv
            .iter()
            .any(|a| a == "--dangerously-skip-permissions"));
        finish(&b, &host);
    }
);

parity!(
    box_brief_is_a_file_naming_the_reports_path_without_host_paths_or_moor_commands,
    covers = ["D-28", "REQ-32", "REQ-34", "C-105"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("brief");
        let marker = format!("GOALMARK-{}", b.w.task_name("g"));
        let l = launched(&b, &name, &marker);
        assert!(l.brief.contains(&marker));
        assert!(l.brief.contains(".moor/reports/"), "{}", l.brief);
        for bad in [
            "moor run",
            "moor report",
            "moor steer",
            "`moor ",
            "--state-dir",
        ] {
            assert!(!l.brief.contains(bad), "brief contains {bad}");
        }
        let rows = moor().list(&b.w, &b.repo.path).ok().clone();
        let mut hosts = vec![
            b.w.root().display().to_string(),
            b.w.state_dir().display().to_string(),
            b.repo.path.display().to_string(),
            b.w.home().display().to_string(),
        ];
        hosts.extend(rows.iter().filter_map(|t| t.worktree.clone()));
        for h in hosts {
            assert!(!l.brief.contains(&h), "brief names host path {h}");
        }
        for word in ["shim", "MOOR_GH", "`gh"] {
            assert!(!l.brief.contains(word), "brief mentions {word}");
        }
        assert!(l.brief.contains("MOOR_PUSH_URL"), "{}", l.brief);
        assert!(
            l.brief.contains(&format!("moor/{name}")),
            "Task branch named"
        );
        assert!(l.brief.contains("hook"), "defers to repo hooks");
        for c in b.w.all_calls() {
            assert!(
                !c.argv.iter().any(|a| a.contains(&marker)),
                "brief text in argv of {}",
                c.tool
            );
        }
        assert!(!pane_command(&b.w).contains(&marker));
        let r = ran(&b);
        assert!(
            !r.text.contains(&marker),
            "brief text reached the pane scrollback"
        );
        finish(&b, &name);
    }
);
