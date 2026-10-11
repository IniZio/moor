use super::*;

parity!(
    bot_mint_scopes_to_one_repo_id_without_caching,
    covers = ["C-55"],
    tags = [],
    {
        let g = app_you_world();
        let fx = g.fx.clone();
        let client_secret = g.setup();
        g.identity("bot");
        g.gh.clear_requests();

        assert_eq!(g.mint_status(), "ok");
        let reqs = g.gh.requests();
        let tokens_path = format!("/app/installations/{}/access_tokens", fx.installation_id);
        let want = [
            "GET /repos/acme/widget/installation".to_string(),
            format!("POST {tokens_path}"),
            "GET /repos/acme/widget".to_string(),
            format!("POST {tokens_path}"),
        ];
        assert_subseq(&reqs, &want.iter().map(String::as_str).collect::<Vec<_>>());

        let posts = g.gh.requests_to("POST", &tokens_path);
        assert_eq!(posts.len(), 2);
        assert!(
            posts.iter().all(|p| p.jwt().is_some()),
            "installation tokens use the App JWT"
        );
        assert_eq!(
            posts[0].json(),
            json!({ "permissions": { "metadata": "read" } })
        );
        assert_eq!(
            posts[1].json(),
            json!({ "repository_ids": [fx.repo_id], "permissions": permissions_write() })
        );
        let repo_req = &g.gh.requests_to("GET", "/repos/acme/widget")[0];
        assert_eq!(repo_req.bearer(), Some(fx.probe_token.as_str()));

        g.gh.clear_requests();
        assert_eq!(g.mint_status(), "ok");
        assert_eq!(
            count(&g.gh.requests(), "POST", &tokens_path),
            2,
            "no caching"
        );
        for token in [&fx.bot_token, &fx.probe_token] {
            assert_secret_absent(&g.w, token, &[]);
        }
        assert_secret_absent(&g.w, &client_secret, &["github/"]);
    }
);

parity!(
    mint_uses_canonical_ids_across_rename_redirects,
    covers = ["C-57", "C-50"],
    tags = [],
    {
        let g = app_you_world();
        let fx = g.fx.clone();
        let mut renamed = fx.clone();
        renamed.name = "renamed".into();
        let target = format!("{}{}", g.gh.url(), renamed.repo_path());
        g.gh.reroute("GET", &fx.repo_path(), vec![Reply::redirect(301, &target)]);
        g.gh.route(
            "GET",
            &renamed.repo_path(),
            vec![Reply::ok(&renamed.repo_json())],
        );
        g.setup();
        g.seed_user("ghr_OLD", None);

        for identity in ["bot", "you"] {
            g.identity(identity);
            g.gh.clear_requests();
            assert_eq!(g.mint_status(), "ok", "{identity}");
            let reqs = g.gh.requests();
            let first = &g.gh.requests_to("GET", &fx.repo_path())[0];
            let second = &g.gh.requests_to("GET", &renamed.repo_path())[0];
            assert!(first.header("authorization").is_some());
            assert_eq!(
                first.header("authorization"),
                second.header("authorization"),
                "{identity}: auth is kept on the same-host redirect"
            );
            let final_post = reqs
                .iter()
                .rfind(|r| {
                    r.method == "POST"
                        && (r.path.contains("access_tokens") || r.path.contains("scoped"))
                })
                .expect("token request");
            let body = final_post.json();
            assert_eq!(body["repository_ids"], json!([fx.repo_id]), "{identity}");
            for name in ["widget", "renamed"] {
                assert!(
                    !final_post.body.contains(name),
                    "{identity}: {name} in {}",
                    final_post.body
                );
            }
        }
    }
);

parity!(
    mint_install_errors_are_actionable_and_others_pass_through,
    covers = ["C-58", "C-37"],
    tags = [],
    {
        let g = app_you_world();
        g.setup();
        g.identity("bot");
        g.installation(404);

        let from_file = g.mint();
        assert_eq!(from_file["status"], "failed");
        assert!(
            from_file["error"].as_str().unwrap().contains("moor-bot"),
            "{from_file}"
        );

        std::fs::remove_file(g.w.state_dir().join("github/app.slug")).expect("rm slug");
        g.gh.clear_requests();
        assert!(g.mint_error().contains("moor-bot"), "slug from GET /app");
        assert!(count(&g.gh.requests(), "GET", "/app") >= 1);

        g.gh.reroute("GET", "/app", vec![Reply::message(404, "Not Found")]);
        let generic = g.mint();
        assert_eq!(generic["status"], "failed");
        assert!(!generic["error"].as_str().unwrap().contains("moor-bot"));

        g.installation(500);
        let other = g.mint_error();
        assert!(other.contains("500"), "{other}");
        assert!(!other.contains("installations/new"));

        g.gh.reroute(
            "GET",
            "/app",
            vec![Reply::ok(&json!({ "slug": "moor-bot" }))],
        );
        g.installation(200);
        let tokens = format!("/app/installations/{}/access_tokens", g.fx.installation_id);
        let denied = "App does not have access to the given target";
        let forbidden = Reply::message(403, denied);
        g.gh.reroute_when("POST", &tokens, "repository_ids", vec![forbidden]);
        let target = g.mint();
        assert_eq!(target["status"], "failed");
        assert!(
            target["error"].as_str().unwrap().contains("moor-bot"),
            "{target}"
        );
    }
);

parity!(
    app_you_scoped_403_target_error_names_install_page,
    covers = ["C-58"],
    tags = ["new_only(ScopedInstallErrorMapped)"],
    {
        let g = app_you_world();
        g.setup();
        g.identity("you");
        g.seed_user("ghr_OLD", None);
        let scoped = format!("/applications/{}/token/scoped", g.fx.client_id);
        let denied = "App does not have access to the given target";
        g.gh.reroute("POST", &scoped, vec![Reply::message(403, denied)]);
        let target = g.mint();
        assert_eq!(target["status"], "failed");
        assert!(
            target["error"].as_str().unwrap().contains("moor-bot"),
            "{target}"
        );
    }
);

parity!(
    gh_you_mint_runs_command_per_mint_and_checks_push,
    covers = ["C-61", "C-37"],
    tags = [],
    {
        let g = app_you_world();
        g.setup();
        g.installation(404);
        g.identity("auto");
        let token = g.secret("ghyou");
        g.w.add_rules(
            "gh",
            [Rule::on(&["auth", "token"]).stdout(format!("{token}\n"))],
        );
        g.seed_user("ghr_DEAD", None);
        g.gh.reroute_when(
            "POST",
            "/login/oauth/access_token",
            "refresh_token",
            vec![Reply::oauth_error("bad_refresh_token")],
        );
        g.gh.clear_requests();

        assert_eq!(g.mint_status(), "ok", "gh-you ignores an expired App login");
        assert_eq!(g.gh_token_calls(), 1);
        assert_eq!(g.mint_status(), "ok");
        assert_eq!(g.gh_token_calls(), 2, "the command runs on every mint");
        let repo_req = &g.gh.requests_to("GET", "/repos/acme/widget")[0];
        assert_eq!(
            repo_req.header("authorization"),
            Some(format!("token {token}").as_str())
        );
        assert_eq!(
            count(&g.gh.requests(), "POST", "/login/oauth/access_token"),
            0
        );

        let mut cannot = g.fx.clone();
        cannot.can_push = false;
        g.gh.reroute(
            "GET",
            &cannot.repo_path(),
            vec![Reply::ok(&cannot.repo_json())],
        );
        assert_eq!(g.mint_status(), "failed", "push permission is required");

        g.gh.reroute("GET", &g.fx.repo_path(), vec![Reply::ok(&g.fx.repo_json())]);
        g.w.prepend_rules("gh", [Rule::on(&["auth", "token"]).exit(1)]);
        assert_eq!(g.mint_status(), "failed", "failing command");
        g.w.prepend_rules("gh", [Rule::on(&["auth", "token"]).stdout("\n")]);
        assert_eq!(g.mint_status(), "failed", "empty output");

        let custom_file = g.w.root().join("custom-token");
        let custom = g.secret("custom");
        write_file(&custom_file, &custom, 0o600);
        let cmd = format!("cat '{}'", custom_file.display());
        g.user(json!({ "github": { "identity": "auto", "you_token_cmd": cmd } }));
        let calls = g.gh_token_calls();
        g.gh.clear_requests();
        assert_eq!(g.mint_status(), "ok");
        assert_eq!(g.gh_token_calls(), calls, "custom command replaces gh");
        let repo_req = &g.gh.requests_to("GET", "/repos/acme/widget")[0];
        assert_eq!(
            repo_req.header("authorization"),
            Some(format!("token {custom}").as_str())
        );
        for s in [token.as_str(), custom.as_str()] {
            assert_secret_absent(&g.w, s, &[]);
        }
    }
);
