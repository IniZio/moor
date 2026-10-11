use super::*;

fn files_named(dir: &Path, name: &str, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            files_named(&p, name, out);
        } else if p.file_name().is_some_and(|n| n == name) {
            out.push(p);
        }
    }
}

fn box_file(w: &World, name: &str) -> Option<String> {
    let mut found = Vec::new();
    files_named(&w.box_root(), name, &mut found);
    assert!(found.len() <= 1, "several {name}: {found:?}");
    found.first().and_then(|p| std::fs::read_to_string(p).ok())
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .map(str::to_lowercase)
        .collect()
}

fn branch_of(w: &World) -> String {
    w.calls("herdr")
        .iter()
        .find(|c| {
            c.argv
                .starts_with(&["worktree".to_string(), "create".to_string()])
        })
        .and_then(|c| c.flag("--branch"))
        .expect("worktree branch")
}

fn box_brief_fx(repo_cfg: Value) -> Fx {
    let mut c = cfg(&[]);
    for (k, v) in repo_cfg.as_object().expect("object") {
        c[k] = v.clone();
    }
    let f = fx(Preset::Sprites, false, json!({}), c);
    let file = f.w.root().join("claude-token");
    std::fs::write(&file, format!("sk-parity-{}", f.w.task_name("tok"))).expect("token file");
    let user = json!({
        "box": { "provider": "sprites" },
        "credentials": { "CLAUDE_CODE_OAUTH_TOKEN": { "file": file } },
    });
    std::fs::write(f.w.config_path(), user.to_string()).expect("user config");
    f
}

fn token_of(f: &Fx) -> String {
    std::fs::read_to_string(f.w.root().join("claude-token")).expect("token")
}

parity!(
    box_brief_states_the_box_contract_and_carries_no_secret,
    covers = ["C-139"],
    tags = [],
    {
        let f = box_brief_fx(json!({}));
        let (name, o) = f.start_with("brf", Some("ship the widget"), Some("ABC-7"));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let brief = box_file(&f.w, "brief.md").expect("brief delivered to the Box");
        let branch = branch_of(&f.w);
        assert!(brief.contains("Goal: ship the widget"), "{brief}");
        assert!(brief.contains("Issue: ABC-7"), "{brief}");
        for marker in [
            "sudo",
            "official",
            "detached",
            "$MOOR_PUSH_URL",
            "Co-Authored-By",
            "commit-msg hook",
            ".moor/reports/",
            "exit code",
            "tail",
        ] {
            assert!(brief.contains(marker), "missing marker {marker}");
        }
        assert!(
            brief.contains(&format!("HEAD:refs/heads/{branch}")),
            "push target names the Task branch {branch}"
        );
        assert!(
            !brief.contains("Setup failed"),
            "no failure note on success"
        );

        assert!(!brief.contains(&token_of(&f)), "token in brief");
        if let Some(nonce) = box_file(&f.w, "proxy-nonce") {
            assert!(!brief.contains(nonce.trim()), "nonce in brief");
        }
        for outside in [
            f.w.root().to_string_lossy().into_owned(),
            f.repo.path.to_string_lossy().into_owned(),
        ] {
            assert!(!brief.contains(&outside), "world path in brief");
        }
        for sub in ["run", "report", "steer", "cp", "gate", "close", "proxy"] {
            assert!(
                !brief.contains(&format!("moor {sub}")),
                "moor {sub} in brief"
            );
        }
        let w = words(&brief);
        assert!(!w.contains(&"gh".to_string()), "brief mentions gh");
        assert!(
            !w.contains(&"proxy".to_string()),
            "brief mentions the proxy"
        );
        f.finish(&name);
    }
);

parity!(
    box_brief_reports_setup_failure_only_when_failed,
    covers = ["C-139"],
    tags = [],
    {
        let f = box_brief_fx(json!({ "setup": ["exit 3"] }));
        let (name, o) = f.start("sfail", Some("go"));
        assert_ne!(o.class, ExitClass::Usage, "{}", o.stderr);
        let brief = box_file(&f.w, "brief.md").expect("brief delivered despite setup failure");
        assert!(brief.contains("Setup failed (exit 3)"), "{brief}");
        assert!(brief.contains(".moor/setup.log"), "{brief}");
        f.finish(&name);
    }
);

parity!(
    start_without_goal_dispatches_no_agent_and_no_proxy,
    covers = ["C-141"],
    tags = [],
    {
        let f = fx(Preset::Sprites, false, json!({}), cfg(&[]));
        let (name, o) = f.start_with("nogoal", None, Some("ABC-7"));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(f.w.calls("claude").is_empty(), "agent launched");
        assert!(
            herdr::calls_of(&f.w, "pane", "run").is_empty(),
            "pane command run"
        );
        assert!(box_file(&f.w, "brief.md").is_none(), "brief");
        assert!(
            f.w.calls("crabbox")
                .iter()
                .all(|c| c.argv.first().map(String::as_str) != Some("tunnel")),
            "tunnel opened"
        );
        let proxy_dir = f.w.state_dir().join("proxy");
        let proxies = std::fs::read_dir(proxy_dir).map_or(0, |d| d.flatten().count());
        assert_eq!(proxies, 0, "proxy state written");
        f.finish(&name);
    }
);

parity!(
    goal_and_model_reach_the_dispatched_box_agent,
    covers = ["C-141"],
    tags = [],
    {
        let f = fx(Preset::Sprites, false, json!({}), cfg(&[]));
        let name = f.w.task_name("mdl");
        let opts = StartOpts {
            goal: Some("tune the widget".to_string()),
            model: Some("sonnet".to_string()),
            ..Default::default()
        };
        let o = moor().start(&f.w, &f.repo.path, &name, &opts);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let brief = box_file(&f.w, "brief.md").expect("brief delivered to the Box");
        assert!(brief.contains("Goal: tune the widget"), "{brief}");
        let pane = herdr::calls_of(&f.w, "pane", "run")
            .last()
            .and_then(|c| c.argv.last().cloned())
            .expect("pane command");
        let argv = pane
            .split("claude --model ")
            .nth(1)
            .expect("claude --model");
        assert!(
            words(argv)
                .iter()
                .find(|w| !w.is_empty())
                .is_some_and(|w| w == "sonnet"),
            "{pane}"
        );
        f.finish(&name);
    }
);
