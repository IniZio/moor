use super::*;

parity!(
    github_app_jwt_claims_and_key_secrecy,
    covers = ["C-49"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let before = now();
        g.setup();
        let after = now();
        let reqs = g.gh.requests();
        assert_eq!(reqs.len(), 2);
        for req in &reqs {
            let (header, claims) = req.jwt().expect("Bearer JWT");
            assert_eq!(header["alg"], "RS256");
            assert_eq!(claims["iss"].to_string().trim_matches('"'), "4242");
            let (iat, exp) = (
                claims["iat"].as_u64().unwrap(),
                claims["exp"].as_u64().unwrap(),
            );
            assert!(iat + 60 >= before && iat + 60 <= after, "iat {iat}");
            assert_eq!(exp - iat, 600, "exp follows now+540, iat now-60");
        }

        g.gh.clear_requests();
        g.wipe_store();
        let junk = "not a pem JUNKKEYMARKER\n";
        let key = g.w.root().join("junk.pem");
        write_file(&key, junk, 0o600);
        let mut opts = g.opts("s3cret");
        opts.key = Some(key);
        let o = g.setup_with(&opts);
        assert_eq!(o.class, ExitClass::Failed);
        assert!(g.gh.requests().is_empty(), "bad key reached GitHub");
        assert!(!o.stderr.contains("JUNKKEYMARKER") && !o.stdout.contains("JUNKKEYMARKER"));
        assert_secret_absent(&g.w, "JUNKKEYMARKER", &["github/"]);
    }
);

parity!(
    github_http_client_headers_and_urls,
    covers = ["C-50"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        g.w.set_env("MOOR_GITHUB_API", &format!("{}/", g.gh.url()));
        g.w.set_env("MOOR_GITHUB_WEB", &format!("{}/", g.gh.url()));
        g.setup();
        let reqs = g.gh.requests();
        assert_eq!(reqs[0].path, "/app", "trailing slash is trimmed");
        for req in &reqs {
            assert_eq!(req.header("user-agent"), Some("moor"));
            assert_eq!(req.header("accept"), Some("application/vnd.github+json"));
            assert_eq!(req.header("x-github-api-version"), Some("2022-11-28"));
            assert_eq!(req.header("content-type"), Some("application/json"));
            assert!(req.bearer().is_some());
        }
        g.gh.clear_requests();
        assert_eq!(g.login().class, ExitClass::Ok);
        let web = g.gh.requests_to("POST", "/login/device/code");
        assert_eq!(web.len(), 1);
        assert_eq!(web[0].header("accept"), Some("application/json"));
        assert_eq!(web[0].header("user-agent"), Some("moor"));
        assert!(web[0].header("authorization").is_none());
        assert_eq!(web[0].json(), json!({ "client_id": g.fx.client_id }));
    }
);

parity!(
    github_http_errors_name_endpoint_status_and_hide_secrets,
    covers = ["C-50"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        let probe = "/repos/acme/widget/installation";
        let long = "x".repeat(300);
        g.gh.reroute("GET", probe, vec![Reply::message(500, &long)]);
        let o = g.setup_with(&g.opts("s3cret"));
        assert_eq!(o.class, ExitClass::Failed);
        assert!(o.stderr.contains(&format!("GET {probe}")), "{}", o.stderr);
        assert!(o.stderr.contains("500"));
        assert!(o.stderr.contains(&"x".repeat(200)), "reason is shown");
        assert!(!o.stderr.contains(&"x".repeat(201)), "reason is truncated");

        let leaky = json!({ "message": "nope", "token": "ghs_LEAKME", "private_key": "PEMLEAK" });
        g.gh.reroute("GET", probe, vec![Reply::json(401, &leaky)]);
        let o = g.setup_with(&g.opts("s3cret"));
        assert_eq!(o.class, ExitClass::Failed);
        assert!(o.stderr.contains("401"));
        for leak in ["ghs_LEAKME", "PEMLEAK"] {
            assert!(!o.stderr.contains(leak) && !o.stdout.contains(leak));
        }

        g.installation(200);
        g.setup();
        let flow = json!({ "error": "device_flow_disabled", "error_description": "turn it on" });
        g.gh.reroute("POST", "/login/device/code", vec![Reply::json(400, &flow)]);
        let o = g.login();
        assert_eq!(o.class, ExitClass::Failed);
        for part in [
            "POST /login/device/code",
            "400",
            "device_flow_disabled",
            "turn it on",
        ] {
            assert!(o.stderr.contains(part), "{part}: {}", o.stderr);
        }
        assert!(snap(&o).user.is_none());
    }
);
