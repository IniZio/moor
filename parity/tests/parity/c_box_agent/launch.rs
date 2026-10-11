use super::*;

parity!(
    box_launch_proxies_then_delivers_then_opens_agent_tab,
    covers = ["A-39", "C-97"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("order");
        let o = go_ok(&b, &name, "build it");
        let task = o.ok().clone();
        let calls = b.w.all_calls();
        assert_order(
            &calls,
            &[
                ("crabbox", &["warmup"]),
                ("crabbox", &["tunnel", "--reverse"]),
                ("crabbox", &["ssh"]),
                ("ssh", &["box-host"]),
                ("herdr", &["tab", "create"]),
                ("herdr", &["pane", "run"]),
            ],
        );
        let slug = crabbox::slug(&b.w).expect("slug");
        let tunnel = crabbox::call(&b.w, "tunnel").expect("tunnel");
        assert_eq!(arg_after(&tunnel, "--id").as_deref(), Some(slug.as_str()));
        let (box_port, host_port) = tunnel_ports(&b.w);
        assert!(box_port > 0 && host_port > 0);
        let tab = herdr::calls_of(&b.w, "tab", "create").remove(0);
        assert_eq!(
            arg_after(&tab, "--workspace"),
            task.workspace_id,
            "agent tab opens in the Task Workspace"
        );
        assert_eq!(arg_after(&tab, "--label"), Some(format!("agent:{name}")));
        assert_eq!(arg_after(&tab, "--cwd"), task.worktree);
        let sessions = moor().sessions(&b.w, &name);
        assert_eq!(sessions.len(), 1, "{sessions:?}");
        assert!(sessions[0].box_pane && sessions[0].live);
        let run = herdr::calls_of(&b.w, "pane", "run").remove(0);
        assert_eq!(run.argv[2], sessions[0].session_id);
        let started: Vec<_> = moor()
            .events(&b.w, &name)
            .into_iter()
            .filter(|e| e.kind == EventKind::SessionStarted)
            .collect();
        assert_eq!(started.len(), 1);
        assert_eq!(
            cred_dirs(&b.w).len(),
            1,
            "files delivered before the pane opens"
        );
        finish(&b, &name);
    }
);

parity!(
    box_agent_without_goal_launches_nothing,
    covers = ["C-110"],
    tags = [],
    {
        let b = bx(false);
        let name = b.w.task_name("nogoal");
        let o = start_with(&b, &name, &start_opts(None, None));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().state, TaskState::Working);
        let calls = b.w.all_calls();
        assert_never_called(&calls, "crabbox", &["tunnel"]);
        assert_never_called(&calls, "herdr", &["tab", "create"]);
        assert_never_called(&calls, "herdr", &["pane", "run"]);
        assert!(b.w.calls("ssh").is_empty(), "nothing delivered");
        assert!(moor().sessions(&b.w, &name).is_empty());
        assert!(cred_dirs(&b.w).is_empty());
        let proxy = b.w.state_dir().join("proxy");
        assert!(
            !proxy.exists() || files_under(&proxy).is_empty(),
            "no proxy without an agent"
        );
        finish(&b, &name);
    }
);

parity!(
    box_launch_delivers_only_nonce_brief_and_shim_with_private_modes,
    covers = ["C-98"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("files");
        let l = launched(&b, &name, "deliver");
        let want = [
            "600 brief.md",
            "600 proxy-nonce",
            "700 .",
            "700 bin",
            "700 bin/gh",
        ];
        assert_eq!(l.files, want, "credential dir layout and modes");
        assert!(!l.nonce.is_empty() && !l.nonce.contains(char::is_whitespace));
        let shim = l.shim.expect("shim delivered for a github.com origin");
        assert!(shim.starts_with("#!/bin/sh"), "{shim}");
        assert!(l.brief.contains("# Task"), "{}", l.brief);
        assert!(
            ssh_stdin_has(&b.w, &l.nonce),
            "nonce travels over ssh stdin"
        );
        assert!(
            ssh_stdin_has(&b.w, "MOOR_GH_API"),
            "shim travels over ssh stdin"
        );
        for c in b.w.all_calls() {
            assert!(
                !c.argv.iter().any(|a| a.contains(&l.nonce)),
                "argv of {}",
                c.tool
            );
        }
        for token in [&b.fx.bot_token, &b.fx.probe_token, &b.fx.scoped_token] {
            assert!(!ssh_stdin_has(&b.w, token), "github token in ssh stdin");
            assert_secret_absent(&b.w, token, &[]);
        }
        b.w.assert_no_new_tmp_creds();
        finish(&b, &name);
    }
);

parity!(
    box_agent_pane_runs_one_line_and_removes_cred_dir_on_any_exit,
    covers = ["C-99", "C-106", "REQ-35"],
    tags = [],
    {
        let b = bx(false);
        for (tag, code, kill) in [("ok", 0, false), ("three", 3, false), ("kill", 0, true)] {
            if kill {
                agent_rule(&b.w, "; kill -9 $$");
            } else {
                agent_exit(&b.w, code);
            }
            let name = b.w.task_name(tag);
            launched(&b, &name, "exit codes");
            let r = ran(&b);
            let marker = exit_marker(&r.text);
            if kill {
                assert!(marker.is_some_and(|m| m != 0), "{tag}: {}", r.text);
            } else {
                assert_eq!(marker, Some(code), "{tag}: {}", r.text);
            }
            assert!(cred_dirs(&b.w).is_empty(), "{tag}: {:?}", cred_dirs(&b.w));
            finish(&b, &name);
        }
        b.w.assert_no_new_tmp_creds();
    }
);

parity!(
    box_agent_gets_proxy_nonce_never_a_real_credential,
    covers = ["C-100", "C-111", "C-104", "C-106"],
    tags = [],
    {
        let b = bx(true);
        let claude_token = format!("SECRET-claude-{}", b.w.task_name("v"));
        b.w.register_secret(&claude_token);
        let file = b.w.root().join("secrets/claude");
        write_file(&file, &claude_token, 0o600);
        set_user(
            &b,
            json!({ "credentials": { "CLAUDE_CODE_OAUTH_TOKEN": { "file": file } } }),
        );
        let name = b.w.task_name("nonce");
        let l = launched(&b, &name, "no secret in the Box");
        let r = ran(&b);
        assert_eq!(r.code, 0, "{}", r.text);
        assert_eq!(
            r.env.get("ANTHROPIC_BASE_URL"),
            Some(&format!("http://127.0.0.1:{}", l.box_port))
        );
        assert_eq!(r.env.get("CLAUDE_CODE_OAUTH_TOKEN"), Some(&l.nonce));
        for var in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
        ] {
            assert!(!r.env.contains_key(var), "{var} reached the agent");
        }
        assert!(
            !r.find.contains("proxy-nonce"),
            "nonce file is gone before claude runs"
        );
        assert_nonce_confined(&b.w, &l.nonce);
        let pem_line = TEST_KEY_PEM.lines().nth(1).expect("pem body");
        let host_secrets = [
            claude_token.as_str(),
            CLIENT_SECRET,
            pem_line,
            &b.fx.bot_token,
            &b.fx.probe_token,
            &b.fx.scoped_token,
            &b.fx.user_access,
            &b.fx.user_refresh,
        ];
        let env_text = r.env.values().cloned().collect::<Vec<_>>().join("\n");
        for s in host_secrets {
            assert_secret_absent(&b.w, s, &["github/"]);
            assert!(!ssh_stdin_has(&b.w, s), "{s} in ssh stdin");
            assert!(!env_text.contains(s), "{s} in agent env");
            assert!(!pane_command(&b.w).contains(s), "{s} in pane command");
        }
        finish(&b, &name);
        assert_nonce_confined(&b.w, &l.nonce);
    }
);
