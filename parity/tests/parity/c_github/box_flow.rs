use super::*;

fn flow_fakes(w: &World) {
    crabbox::install(w);
    herdr::install(w);
    claude::install(w);
    w.add_rules(
        "ssh",
        [Rule::any()
            .exec("cd '{box}' && {last}")
            .rewrite("/tmp/", "{box}/tmp/")
            .capture_stdin()],
    );
    let ssh_bin = w.tool_path("ssh");
    w.prepend_rules(
        "crabbox",
        [
            Rule::on(&["run"])
                .contains("MOOR_WORKDIR")
                .stdout("MOOR_WORKDIR={box}/work\n"),
            Rule::on(&["run"])
                .exec("{post}")
                .rewrite("/tmp/", "{box}/tmp/")
                .stderr("running on 192.0.2.10 {post}\n"),
            Rule::on(&["ssh"]).stdout(format!("'{}' 'box-host'\n", ssh_bin.display())),
            Rule::on(&["tunnel"])
                .stdout("{\"box\":1,\"host\":1}\n")
                .block(),
        ],
    );
}

fn wait_proxy_gone(w: &World) {
    let dir = w.state_dir().join("proxy");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        let any_json = std::fs::read_dir(&dir).is_ok_and(|d| {
            d.flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == "json"))
        });
        if !any_json {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("proxy supervisor did not exit after close");
}

fn box_start(g: &Gx, name: &str) -> Outcome<parity::adapter::TaskView> {
    write_file(
        &g.repo.path.join(".moor.yaml"),
        &json!({ "agent": { "location": "box" } }).to_string(),
        0o644,
    );
    let opts = StartOpts {
        goal: Some("go".into()),
        location: Some("box".into()),
        ..Default::default()
    };
    moor().start(&g.w, &g.repo.path, name, &opts)
}

fn identity_events(g: &Gx, name: &str) -> Vec<Value> {
    moor()
        .events(&g.w, name)
        .into_iter()
        .filter(|e| e.kind == EventKind::Other("github_identity".into()))
        .map(|e| e.payload)
        .collect()
}

parity!(
    box_start_preflight_mint_failure_acquires_no_box,
    covers = ["C-56"],
    tags = [],
    {
        let g = app_you_world();
        flow_fakes(&g.w);
        g.identity("bot");
        let name = g.w.task_name("nopreflight");
        let o = box_start(&g, &name);
        assert_eq!(o.class, ExitClass::Failed, "{}", o.stderr);
        let kinds: Vec<EventKind> = moor()
            .events(&g.w, &name)
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert!(kinds.contains(&EventKind::CredentialFailed), "{kinds:?}");
        assert_never_called(&g.w.all_calls(), "crabbox", &["warmup"]);
        assert_never_called(&g.w.all_calls(), "herdr", &["pane", "run"]);
        assert!(g.gh.requests().is_empty(), "no App, no HTTP");
        g.no_gh_fallback();
    }
);

parity!(
    proxy_records_github_identity_mode_only,
    covers = ["C-63", "C-60"],
    tags = [],
    {
        let g = app_you_world();
        flow_fakes(&g.w);
        g.setup();
        g.identity("bot");
        let name = g.w.task_name("ident");
        let o = box_start(&g, &name);
        assert_eq!(o.class, ExitClass::Ok, "{}{}", o.stdout, o.stderr);
        let events = identity_events(&g, &name);
        assert_eq!(events, [json!({ "mode": "bot" })], "one event, mode only");
        let closed = moor().close(&g.w, &g.repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        wait_proxy_gone(&g.w);
        for token in [&g.fx.bot_token, &g.fx.probe_token] {
            assert_secret_absent(&g.w, token, &[]);
        }
    }
);

parity!(
    proxy_records_no_identity_without_github_repo,
    covers = ["C-63"],
    tags = [],
    {
        let g = gx();
        g.gh.script_repo(&g.fx);
        flow_fakes(&g.w);
        let name = g.w.task_name("nogh");
        let o = box_start(&g, &name);
        assert_eq!(o.class, ExitClass::Ok, "{}{}", o.stdout, o.stderr);
        assert!(identity_events(&g, &name).is_empty());
        let closed = moor().close(&g.w, &g.repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        wait_proxy_gone(&g.w);
    }
);
