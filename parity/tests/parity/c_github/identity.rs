use super::*;

fn probe_case(g: &Gx, identity: &str, installation: u16) -> Value {
    g.identity(identity);
    g.installation(installation);
    g.gh.clear_requests();
    g.resolved()
}

parity!(
    resolved_identity_modes_and_app_installed_probe,
    covers = ["C-35", "C-59", "C-60"],
    tags = [],
    {
        let g = app_you_world();
        g.w.add_rules(
            "gh",
            [Rule::on(&["auth", "token"]).stdout("ghp_HOSTTOKEN\n")],
        );

        g.identity("auto");
        let no_app = g.resolved();
        assert_eq!(no_app["mode"], "gh-you");
        assert_eq!(no_app["app_installed"], "n/a");
        assert!(no_app["token_source"].as_str().unwrap().contains("gh"));
        assert!(
            g.gh.requests().is_empty(),
            "auto without an App makes no HTTP call"
        );

        g.identity("bot");
        let bot_no_app = g.resolved();
        assert_eq!(
            (&bot_no_app["mode"], &bot_no_app["app_installed"]),
            (&json!("bot"), &json!("n/a"))
        );

        g.setup();
        let auto_yes = probe_case(&g, "auto", 200);
        assert_eq!(
            (&auto_yes["mode"], &auto_yes["app_installed"]),
            (&json!("app-you"), &json!("yes"))
        );
        let probes = g.gh.requests_to("GET", "/repos/acme/widget/installation");
        assert!(!probes.is_empty() && probes.iter().all(|p| p.jwt().is_some()));

        let auto_no = probe_case(&g, "auto", 404);
        assert_eq!(
            (&auto_no["mode"], &auto_no["app_installed"]),
            (&json!("gh-you"), &json!("no"))
        );

        let gh_calls = g.gh_token_calls();
        let broken = probe_case(&g, "auto", 500);
        assert!(broken["mode"].is_null(), "{broken}");
        assert_eq!(broken["app_installed"], "unknown");
        let err = broken["error"].as_str().expect("resolution error");
        assert!(err.contains("acme/widget") && err.contains("500"), "{err}");
        assert_eq!(
            g.gh_token_calls(),
            gh_calls,
            "a failed probe must not fall back to gh"
        );

        for (identity, mode) in [("bot", "bot"), ("you", "app-you")] {
            for (status, installed) in [(200, "yes"), (404, "no"), (500, "unknown")] {
                let r = probe_case(&g, identity, status);
                assert_eq!(r["mode"], mode, "{identity} {status}");
                assert_eq!(r["app_installed"], installed, "{identity} {status}");
            }
        }
        let sources: Vec<String> = ["bot", "you", "auto"]
            .iter()
            .map(|i| {
                let status = if *i == "auto" { 404 } else { 200 };
                probe_case(&g, i, status)["token_source"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert!(sources[0] != sources[1] && sources[1] != sources[2] && sources[0] != sources[2]);

        g.user(json!({ "github": { "identity": "auto", "you_token_cmd": "echo custom-cmd" } }));
        g.installation(404);
        let custom = g.resolved();
        assert!(custom["token_source"]
            .as_str()
            .unwrap()
            .contains("echo custom-cmd"));
    }
);

parity!(
    resolved_identity_unknown_for_non_github_origin,
    covers = ["C-35"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        g.setup();
        g.identity("auto");
        let r = g.resolved();
        assert!(r["mode"].is_null(), "{r}");
        assert_eq!(r["app_installed"], "unknown");
        assert!(r["token_source"].is_null());
        assert!(!r["error"].as_str().unwrap_or("").is_empty());
    }
);

parity!(
    mint_check_skipped_failed_and_exit_stays_ok,
    covers = ["C-37"],
    tags = [],
    {
        let g = app_you_world();
        g.w.add_rules(
            "gh",
            [Rule::on(&["auth", "token"]).stdout("ghp_HOSTTOKEN\n")],
        );
        g.identity("auto");
        let none = g.mint();
        assert_eq!(none["status"], "skipped");
        assert!(
            g.gh.requests().is_empty() && g.gh_token_calls() == 0,
            "no App: nothing minted"
        );

        g.setup();
        g.identity("bot");
        g.git(&["remote", "remove", "origin"]);
        assert_eq!(g.mint_status(), "skipped", "no origin remote");
        g.git(&["remote", "add", "origin", "https://example.com/acme/widget"]);
        assert_eq!(g.mint_status(), "skipped", "origin is not github.com");
        g.git(&[
            "remote",
            "set-url",
            "origin",
            "https://github.com/acme/widget",
        ]);
        assert_eq!(g.mint_status(), "ok");

        let tokens_path = format!("/app/installations/{}/access_tokens", g.fx.installation_id);
        g.gh.reroute_when(
            "POST",
            &tokens_path,
            "repository_ids",
            vec![Reply::message(500, "boom")],
        );
        let failed = g.mint();
        assert_eq!(failed["status"], "failed");
        assert!(
            failed["error"].as_str().unwrap().contains("500"),
            "{failed}"
        );
    }
);
