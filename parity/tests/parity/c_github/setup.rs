use super::*;

parity!(
    github_setup_requires_app_client_and_key,
    covers = ["C-43"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let full = g.opts("s3cret");
        let cases = [
            GithubSetupOpts {
                app_id: None,
                ..full.clone()
            },
            GithubSetupOpts {
                client_id: None,
                ..full.clone()
            },
            GithubSetupOpts {
                key: None,
                ..full.clone()
            },
        ];
        for opts in cases {
            let o = g.setup_with(&opts);
            assert_eq!(o.class, ExitClass::Usage, "{}", o.stderr);
            assert!(!snap(&o).exists(), "usage error created the store");
        }
        assert!(g.gh.requests().is_empty());
    }
);

parity!(
    github_setup_accepts_repo_forms_and_rejects_others,
    covers = ["C-43"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let forms = [
            "acme/widget",
            "acme/widget.git",
            "https://github.com/acme/widget",
            "https://github.com/acme/widget.git",
            "git@github.com:acme/widget.git",
        ];
        for form in forms {
            let mut opts = g.opts("s3cret");
            opts.repo = Some(form.into());
            opts.json = true;
            let o = g.setup_with(&opts);
            assert_eq!(o.class, ExitClass::Ok, "{form}: {}", o.stderr);
            let v = assert_pure_json(&o.stdout);
            assert_eq!(v["repo"], "acme/widget", "{form}");
            assert_eq!(v["installation_id"], g.fx.installation_id, "{form}");
        }
        let probe = "/repos/acme/widget/installation";
        assert_eq!(count(&g.gh.requests(), "GET", probe), forms.len());
        let before = g.gh.requests().len();
        for bad in ["widget", "https://gitlab.com/acme/widget", "acme/widget/x"] {
            let mut opts = g.opts("s3cret");
            opts.repo = Some(bad.into());
            let o = g.setup_with(&opts);
            assert_eq!(o.class, ExitClass::Failed, "{bad}");
        }
        assert_eq!(g.gh.requests().len(), before, "bad repo reached GitHub");
    }
);

parity!(
    github_setup_default_repo_is_origin,
    covers = ["C-43"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        g.github_origin();
        let mut opts = g.opts("s3cret");
        opts.repo = None;

        g.git(&["remote", "remove", "origin"]);
        let none = g.setup_with(&opts);
        assert_eq!(none.class, ExitClass::Failed, "{}", none.stderr);
        assert!(!snap(&none).exists());

        g.git(&["remote", "add", "origin", "https://example.com/acme/widget"]);
        let foreign = g.setup_with(&opts);
        assert_eq!(foreign.class, ExitClass::Failed, "{}", foreign.stderr);
        assert!(!snap(&foreign).exists());
        assert!(g.gh.requests().is_empty());

        g.git(&[
            "remote",
            "set-url",
            "origin",
            "https://github.com/acme/widget",
        ]);
        let ok = g.setup_with(&opts);
        assert_eq!(ok.class, ExitClass::Ok, "{}", ok.stderr);
        let probe = g.gh.requests_to("GET", "/repos/acme/widget/installation");
        assert_eq!(probe.len(), 1);
    }
);

parity!(
    github_setup_client_secret_intake_never_argv,
    covers = ["C-44"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let (env_secret, stdin_secret) = (g.secret("env"), g.secret("stdin"));
        let stored = |g: &Gx| g.store().app.expect("app.json")["client_secret"].clone();

        let mut opts = g.opts(&env_secret);
        assert_eq!(g.setup_with(&opts).class, ExitClass::Ok);
        assert_eq!(stored(&g), env_secret);

        opts.secret_env = None;
        opts.secret_stdin = Some(format!("  {stdin_secret}  \nSECOND-LINE-IGNORED\n"));
        assert_eq!(g.setup_with(&opts).class, ExitClass::Ok);
        assert_eq!(stored(&g), stdin_secret);

        opts.secret_env = Some(env_secret.clone());
        assert_eq!(g.setup_with(&opts).class, ExitClass::Ok);
        assert_eq!(stored(&g), env_secret, "env must win over stdin");

        let empties = [
            (Some(""), None),
            (None, Some("")),
            (None, Some("   \n")),
            (Some("  "), Some("ignored\n")),
        ];
        for (env, stdin) in empties {
            g.wipe_store();
            let mut o = g.opts("x");
            o.secret_env = env.map(str::to_string);
            o.secret_stdin = stdin.map(str::to_string);
            let r = g.setup_with(&o);
            assert_eq!(r.class, ExitClass::Failed, "{env:?} {stdin:?}");
            assert!(snap(&r).app.is_none(), "empty secret stored an App");
        }

        g.wipe_store();
        let mut flagged = g.opts(&env_secret);
        flagged.extra_args = vec!["--client-secret".into(), "FLAGVALUE".into()];
        let r = g.setup_with(&flagged);
        assert_eq!(r.class, ExitClass::Usage, "{}", r.stderr);
        assert!(!snap(&r).exists());

        g.setup_with(&g.opts(&env_secret));
        for s in [&env_secret, &stdin_secret] {
            assert_secret_absent(&g.w, s, &["github/"]);
        }
    }
);

parity!(
    github_setup_refuses_loose_key_files,
    covers = ["C-45"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let secret = g.secret("client");
        let marker = TEST_KEY_PEM.lines().nth(1).expect("pem body");
        for mode in [0o640, 0o604, 0o644, 0o660, 0o666] {
            let mut opts = g.opts(&secret);
            opts.key = Some(g.key(mode));
            let o = g.setup_with(&opts);
            assert_eq!(o.class, ExitClass::Failed, "mode {mode:o}");
            assert!(!snap(&o).exists(), "mode {mode:o} created the store");
            assert!(!o.stdout.contains(marker) && !o.stderr.contains(marker));
            assert!(!o.stderr.contains(&secret));
        }
        assert!(g.gh.requests().is_empty(), "loose key reached GitHub");
        for mode in [0o600, 0o400] {
            let mut opts = g.opts(&secret);
            opts.key = Some(g.key(mode));
            let o = g.setup_with(&opts);
            assert_eq!(o.class, ExitClass::Ok, "mode {mode:o}: {}", o.stderr);
            assert_eq!(snap(&o).dir_mode, Some(0o700));
        }
        assert_secret_absent(&g.w, marker, &["github/"]);
    }
);

parity!(
    github_store_layout_modes_and_parse_errors,
    covers = ["C-46"],
    tags = [],
    {
        let g = gx();
        g.github_origin();
        g.gh.script_repo(&g.fx);
        g.identity("you");
        let github_dir = g.w.state_dir().join("github");
        std::fs::create_dir_all(&github_dir).expect("mkdir");
        std::fs::set_permissions(&github_dir, std::fs::Permissions::from_mode(0o755))
            .expect("chmod");

        let secret = g.setup();
        let s = g.store();
        assert_eq!(s.dir_mode, Some(0o700), "dir mode is enforced");
        assert_eq!(s.pem.as_deref(), Some(TEST_KEY_PEM.as_bytes()));
        assert_eq!(s.slug.as_deref(), Some(g.fx.slug.as_str()));
        let app = s.app.expect("app.json");
        assert_eq!(app["app_id"], "4242");
        assert_eq!(app["client_id"], g.fx.client_id);
        assert_eq!(app["client_secret"], secret);

        let login = g.login();
        assert_eq!(login.class, ExitClass::Ok, "{}", login.stderr);
        assert_eq!(g.mint_status(), "ok");
        let s = g.store();
        for name in ["app.pem", "app.json", "app.slug", "user.json", "user.lock"] {
            assert_eq!(s.file_mode(name), Some(0o600), "{name}");
        }
        assert!(s.leftover_tmp().is_empty(), "{:?}", s.leftover_tmp());
        assert_eq!(s.dir_mode, Some(0o700));

        write_file(&github_dir.join("app.json"), "{ \"x\": TAILMARKER", 0o600);
        let broken = g.login();
        assert_eq!(broken.class, ExitClass::Failed);
        assert!(broken.stderr.contains("app.json"), "{}", broken.stderr);
        assert!(!broken.stderr.contains("TAILMARKER") && !broken.stdout.contains("TAILMARKER"));

        let marker = TEST_KEY_PEM.lines().nth(1).expect("pem body");
        assert_secret_absent(&g.w, marker, &["github/"]);
        assert_secret_absent(&g.w, &g.fx.user_refresh, &["github/"]);
    }
);

parity!(
    github_setup_flow_order_and_outputs,
    covers = ["C-47"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let mut opts = g.opts("s3cret");
        opts.json = true;
        let o = g.setup_with(&opts);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let v = assert_pure_json(&o.stdout);
        let expected_store = g.w.state_dir().join("github");
        assert_eq!(v["store"], expected_store.to_string_lossy().as_ref());
        assert_eq!(v["app_id"], "4242");
        assert_eq!(v["repo"], "acme/widget");
        assert_eq!(v["installation_id"], g.fx.installation_id);
        let reqs = g.gh.requests();
        assert_eq!(
            seq(&reqs),
            ["GET /app", "GET /repos/acme/widget/installation"]
        );
        assert!(reqs.iter().all(|r| r.jwt().is_some()), "App JWT on both");
        assert_eq!(snap(&o).slug.as_deref(), Some("moor-bot"));

        opts.json = false;
        let text = g.setup_with(&opts);
        assert_eq!(text.class, ExitClass::Ok);
        assert!(!text.stdout.trim().is_empty());

        g.wipe_store();
        g.gh.reroute("GET", "/app", vec![Reply::message(404, "Not Found")]);
        let no_slug = g.setup_with(&opts);
        assert_eq!(no_slug.class, ExitClass::Ok, "slug lookup is best effort");
        assert!(snap(&no_slug).slug.is_none());
    }
);

parity!(
    github_setup_not_installed_keeps_app_and_names_slug,
    covers = ["C-48"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let probe = "/repos/acme/widget/installation";
        let denied = "Resource not accessible: does not have access to the given target";
        let cases = [
            Reply::message(404, "Not Found"),
            Reply::message(403, denied),
        ];
        for reply in cases {
            g.wipe_store();
            g.gh.reroute("GET", probe, vec![reply]);
            let o = g.setup_with(&g.opts("s3cret"));
            assert_eq!(o.class, ExitClass::Failed);
            assert!(snap(&o).app.is_some(), "App must stay saved");
            assert!(o.stderr.contains("moor-bot"), "{}", o.stderr);
        }

        g.wipe_store();
        g.gh.reroute("GET", "/app", vec![Reply::message(404, "Not Found")]);
        let o = g.setup_with(&g.opts("s3cret"));
        assert_eq!(o.class, ExitClass::Failed);
        assert!(snap(&o).app.is_some());
        assert!(!o.stderr.contains("moor-bot"));

        g.wipe_store();
        g.gh.reroute(
            "GET",
            "/app",
            vec![Reply::ok(&json!({ "slug": "moor-bot" }))],
        );
        g.gh.reroute("GET", probe, vec![Reply::message(500, "boom")]);
        let other = g.setup_with(&g.opts("s3cret"));
        assert_eq!(other.class, ExitClass::Failed);
        assert!(snap(&other).app.is_some());
        assert!(other.stderr.contains("500"), "{}", other.stderr);
        assert!(!other.stderr.contains("installations/new"));
    }
);
