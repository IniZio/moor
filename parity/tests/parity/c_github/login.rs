use super::*;

parity!(
    github_login_device_flow_saves_only_refresh,
    covers = ["C-51"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let client_secret = g.setup();
        let stale = g.secret("staleaccess");
        g.seed_user("ghr_STALE", Some((&stale, now() + 99_999)));
        let poll = [
            Reply::oauth_error("authorization_pending"),
            Reply::ok(&json!({ "error": "slow_down", "interval": 0 })),
            Reply::ok(&g.fx.tokens_json()),
        ];
        g.gh.reroute_when(
            "POST",
            "/login/oauth/access_token",
            "device_code",
            poll.to_vec(),
        );
        g.gh.clear_requests();

        let o = g.login();
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(
            o.stderr.contains("PARI-TY00"),
            "user code shown: {}",
            o.stderr
        );
        assert!(o.stderr.contains("https://github.com/login/device"));
        assert!(!o.stdout.trim().is_empty());

        let reqs = g.gh.requests();
        assert_eq!(seq(&reqs)[0], "POST /login/device/code");
        assert_eq!(reqs[0].json(), json!({ "client_id": g.fx.client_id }));
        let polls = g.gh.requests_to("POST", "/login/oauth/access_token");
        assert_eq!(polls.len(), 3, "pending and slow_down keep polling");
        for p in &polls {
            assert_eq!(
                p.json(),
                json!({
                    "client_id": g.fx.client_id,
                    "device_code": "dc_parity",
                    "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
                })
            );
        }
        let user = snap(&o).user.clone().expect("user.json");
        assert_eq!(user["refresh_token"], g.fx.user_refresh);
        assert!(user.get("access_token").is_none(), "cached access dropped");
        assert!(user.get("access_expires_at").is_none());
        assert_eq!(snap(&o).file_mode("user.json"), Some(0o600));
        let expires = user["expires_at"].as_u64().expect("refresh expiry");
        assert!(expires.abs_diff(now() + 15_552_000) < 120, "{expires}");
        assert_secret_absent(&g.w, &g.fx.user_access, &[]);
        assert_secret_absent(&g.w, &stale, &[]);
        assert_secret_absent(&g.w, &g.fx.user_refresh, &["github/"]);
        assert_secret_absent(&g.w, &client_secret, &["github/"]);
    }
);

parity!(github_login_failure_modes, covers = ["C-51"], tags = [], {
    let g = gx();
    g.gh.script_repo(&g.fx);
    let no_app = g.login();
    assert_eq!(no_app.class, ExitClass::Failed);
    assert!(
        g.gh.requests().is_empty(),
        "login without an App hit GitHub"
    );

    g.setup();
    let terminal = ["access_denied", "expired_token", "incorrect_device_code"];
    for error in terminal {
        let reply = Reply::oauth_error(error);
        g.gh.reroute_when(
            "POST",
            "/login/oauth/access_token",
            "device_code",
            vec![reply],
        );
        let o = g.login();
        assert_eq!(o.class, ExitClass::Failed, "{error}");
        assert!(o.stderr.contains(error), "{error}: {}", o.stderr);
        assert!(snap(&o).user.is_none(), "{error} wrote user.json");
    }

    let expired = json!({
        "device_code": "dc_parity", "user_code": "PARI-TY00",
        "verification_uri": "https://github.com/login/device",
        "expires_in": 0, "interval": 0,
    });
    g.gh.reroute("POST", "/login/device/code", vec![Reply::ok(&expired)]);
    g.gh.clear_requests();
    let o = g.login();
    assert_eq!(o.class, ExitClass::Failed);
    assert!(snap(&o).user.is_none());
    assert!(g
        .gh
        .requests_to("POST", "/login/oauth/access_token")
        .is_empty());
});

parity!(
    github_has_no_refresh_loop_subcommand,
    covers = ["C-52"],
    tags = [],
    {
        let g = gx();
        let o = moor().github_raw(&g.w, &g.repo.path, &["refresh-loop"]);
        assert_eq!(o.class, ExitClass::Usage, "{}", o.stderr);
        assert!(g.gh.requests().is_empty());
    }
);

parity!(
    app_you_mint_refreshes_rotates_and_scopes_by_id,
    covers = ["C-53", "C-54", "C-37"],
    tags = [],
    {
        let g = app_you_world();
        let fx = g.fx.clone();
        let refresh = Reply::ok(&fx.tokens("ghr_NEW", 28800));
        g.gh.reroute_when(
            "POST",
            "/login/oauth/access_token",
            "refresh_token",
            vec![refresh],
        );
        let client_secret = g.setup();
        g.identity("you");
        g.seed_user("ghr_OLD", None);
        g.gh.clear_requests();

        assert_eq!(g.mint_status(), "ok");
        let reqs = g.gh.requests();
        let token_path = "/login/oauth/access_token";
        let scoped_path = format!("/applications/{}/token/scoped", fx.client_id);
        let want = [
            format!("POST {token_path}"),
            "GET /repos/acme/widget".to_string(),
            format!("POST {scoped_path}"),
        ];
        assert_subseq(&reqs, &want.iter().map(String::as_str).collect::<Vec<_>>());

        let refresh_req = &g.gh.requests_to("POST", token_path)[0];
        assert_eq!(
            refresh_req.json(),
            json!({
                "client_id": fx.client_id,
                "client_secret": client_secret,
                "grant_type": "refresh_token",
                "refresh_token": "ghr_OLD",
            })
        );
        let repo_req = &g.gh.requests_to("GET", "/repos/acme/widget")[0];
        assert_eq!(repo_req.bearer(), Some(fx.user_access.as_str()));
        let scoped = &g.gh.requests_to("POST", &scoped_path)[0];
        assert_eq!(
            scoped.basic().as_deref(),
            Some(format!("{}:{client_secret}", fx.client_id).as_str())
        );
        assert_eq!(
            scoped.json(),
            json!({
                "access_token": fx.user_access,
                "target_id": fx.owner_id,
                "repository_ids": [fx.repo_id],
                "permissions": permissions_write(),
            })
        );

        let s = g.store();
        let user = s.user.clone().expect("user.json");
        assert_eq!(user["refresh_token"], "ghr_NEW", "rotated token saved");
        assert_eq!(user["access_token"], fx.user_access);
        let at = user["access_expires_at"].as_u64().expect("access expiry");
        assert!(at.abs_diff(now() + 28800) < 120, "{at}");
        assert_eq!(s.file_mode("user.json"), Some(0o600));
        assert_eq!(s.file_mode("user.lock"), Some(0o600));

        g.gh.clear_requests();
        assert_eq!(g.mint_status(), "ok");
        let again = g.gh.requests();
        assert_eq!(count(&again, "POST", token_path), 0, "cached access reused");
        let repo_req = &g.gh.requests_to("GET", "/repos/acme/widget")[0];
        assert_eq!(repo_req.bearer(), Some(fx.user_access.as_str()));
        assert_eq!(
            count(&again, "POST", &scoped_path),
            1,
            "scoped token is not cached"
        );

        assert_secret_absent(&g.w, &fx.scoped_token, &[]);
        for s in [&client_secret, &fx.user_access, "ghr_NEW"] {
            assert_secret_absent(&g.w, s, &["github/"]);
        }
    }
);

parity!(
    app_you_refresh_token_rotation_is_never_replayed,
    covers = ["C-53"],
    tags = [],
    {
        let g = app_you_world();
        let fx = g.fx.clone();
        let replies = vec![
            Reply::ok(&fx.tokens("ghr_ONE", 100)),
            Reply::ok(&fx.tokens("ghr_TWO", 100)),
        ];
        let token_path = "/login/oauth/access_token";
        g.gh.reroute_when("POST", token_path, "refresh_token", replies);
        g.setup();
        g.identity("you");
        g.seed_user("ghr_OLD", Some(("ghu_NEARLY_EXPIRED", now() + 100)));
        g.gh.clear_requests();

        assert_eq!(g.mint_status(), "ok");
        assert_eq!(g.mint_status(), "ok");
        let bodies: Vec<Value> =
            g.gh.requests_to("POST", token_path)
                .iter()
                .map(Request::json)
                .collect();
        assert_eq!(bodies.len(), 2, "access within the margin is refreshed");
        assert_eq!(bodies[0]["refresh_token"], "ghr_OLD");
        assert_eq!(
            bodies[1]["refresh_token"], "ghr_ONE",
            "rotated token used next"
        );
        assert_eq!(g.store().user.expect("user")["refresh_token"], "ghr_TWO");
    }
);

parity!(
    app_you_concurrent_mints_share_one_rotation,
    covers = ["C-53"],
    tags = [],
    {
        let g = app_you_world();
        let fx = g.fx.clone();
        let replies = vec![
            Reply::ok(&fx.tokens("ghr_ONE", 28800)),
            Reply::oauth_error("bad_refresh_token"),
        ];
        let token_path = "/login/oauth/access_token";
        g.gh.reroute_when("POST", token_path, "refresh_token", replies);
        g.setup();
        g.identity("you");
        g.seed_user("ghr_OLD", None);
        g.gh.clear_requests();

        let statuses: Vec<String> = std::thread::scope(|s| {
            let jobs: Vec<_> = (0..2).map(|_| s.spawn(|| g.mint_status())).collect();
            jobs.into_iter()
                .map(|j| j.join().expect("env thread"))
                .collect()
        });
        assert_eq!(statuses, ["ok", "ok"]);
        assert_eq!(count(&g.gh.requests(), "POST", token_path), 1);
        assert_eq!(g.store().user.expect("user")["refresh_token"], "ghr_ONE");
    }
);

parity!(
    app_you_login_problems_fail_without_gh_fallback,
    covers = ["C-53", "C-37"],
    tags = [],
    {
        let g = app_you_world();
        let token_path = "/login/oauth/access_token";
        let scoped_path = format!("/applications/{}/token/scoped", g.fx.client_id);
        g.setup();
        g.identity("you");
        g.w.add_rules(
            "gh",
            [Rule::on(&["auth", "token"]).stdout("ghp_HOSTTOKEN\n")],
        );
        g.gh.clear_requests();

        assert_eq!(g.mint_status(), "failed", "not logged in");
        assert_eq!(count(&g.gh.requests(), "POST", token_path), 0);

        g.seed_user("ghr_DEAD", None);
        let dead = Reply::oauth_error("bad_refresh_token");
        g.gh.reroute_when("POST", token_path, "refresh_token", vec![dead]);
        g.gh.clear_requests();
        let rejected = g.mint();
        assert_eq!(rejected["status"], "failed");
        assert!(
            rejected["error"].as_str().unwrap().contains("login"),
            "{rejected}"
        );
        assert_eq!(count(&g.gh.requests(), "POST", &scoped_path), 0);
        assert_eq!(g.store().user.expect("user")["refresh_token"], "ghr_DEAD");

        let other = Reply::oauth_error("incorrect_client_credentials");
        g.gh.reroute_when("POST", token_path, "refresh_token", vec![other]);
        let failed = g.mint();
        assert_eq!(failed["status"], "failed");
        assert!(failed["error"]
            .as_str()
            .unwrap()
            .contains("incorrect_client_credentials"));
        g.no_gh_fallback();
        assert_secret_absent(&g.w, "ghp_HOSTTOKEN", &[]);
    }
);
