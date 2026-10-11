use super::*;

parity!(
    start_local_container_warmup_argv,
    covers = ["B-70", "B-90"],
    tags = [],
    {
        let (w, repo) = setup(Preset::LocalContainer, json!({}), false);
        write_repo(&repo, json!({ "egress": ["example.com"] }));
        let name = w.task_name("lc");
        let task = started(&w, &repo, &name);
        let slug = crabbox::slug(&w).expect("warmup slug");
        assert!(slug.contains(&name), "slug {slug} embeds the task name");
        assert_ready_handle(&task, "local-container", &slug);
        let warm = assert_warmup(&w, "local-container", &slug, "standard");
        assert_eq!(
            env_of(&warm, "CRABBOX_NAMESPACE_SIZE").as_deref(),
            Some("S")
        );
        assert_eq!(warm.flag("--sprites-network-allow"), None);
        assert_eq!(env_of(&warm, "CRABBOX_NAMESPACE_EGRESS_DOMAINS"), None);
        assert_eq!(env_of(&warm, "CRABBOX_STATIC_ID"), None);
        assert_eq!(env_of(&warm, "CRABBOX_STATIC_PORT"), None);
        assert_eq!(
            std::fs::canonicalize(&warm.cwd).ok(),
            task.worktree
                .as_deref()
                .and_then(|p| std::fs::canonicalize(p).ok())
        );
        assert_run_targets(&w, "local-container", &slug);
        let events: Vec<_> = moor()
            .events(&w, &name)
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert!(events.contains(&EventKind::BoxReady), "{events:?}");
    }
);

parity!(
    start_namespace_devbox_size_and_egress_env,
    covers = ["B-101"],
    tags = [],
    {
        let (w, repo) = setup(Preset::NamespaceDevbox, json!({}), true);
        write_repo(
            &repo,
            json!({ "box": { "size": "large" }, "egress": ["example.com"] }),
        );
        let name = w.task_name("ns");
        let task = started(&w, &repo, &name);
        let slug = crabbox::slug(&w).expect("warmup slug");
        assert_ready_handle(&task, "namespace-devbox", &slug);
        let warm = assert_warmup(&w, "namespace-devbox", &slug, "large");
        assert_eq!(
            env_of(&warm, "CRABBOX_NAMESPACE_SIZE").as_deref(),
            Some("L")
        );
        let domains = env_of(&warm, "CRABBOX_NAMESPACE_EGRESS_DOMAINS").expect("egress domains");
        assert!(domains.split(',').any(|d| d == "example.com"), "{domains}");
        assert_eq!(warm.flag("--sprites-network-allow"), None);
        assert_eq!(env_of(&warm, "CRABBOX_STATIC_ID"), None);
        assert_run_targets(&w, "namespace-devbox", &slug);
    }
);

parity!(
    start_namespace_devbox_without_opt_in_has_no_egress_env,
    covers = ["B-101"],
    tags = [],
    {
        let (w, repo) = setup(Preset::NamespaceDevbox, json!({}), false);
        write_repo(&repo, json!({ "egress": ["example.com"] }));
        let name = w.task_name("nsq");
        started(&w, &repo, &name);
        let warm = verb_calls(&w, "warmup").remove(0);
        assert_eq!(env_of(&warm, "CRABBOX_NAMESPACE_EGRESS_DOMAINS"), None);
        assert_eq!(
            env_of(&warm, "CRABBOX_NAMESPACE_SIZE").as_deref(),
            Some("S")
        );
    }
);

parity!(
    start_sprites_passes_network_allow,
    covers = ["B-100"],
    tags = [],
    {
        let (w, repo) = setup(Preset::Sprites, json!({}), false);
        write_repo(&repo, json!({ "egress": ["example.com"] }));
        let name = w.task_name("sp");
        let task = started(&w, &repo, &name);
        let slug = crabbox::slug(&w).expect("warmup slug");
        assert_ready_handle(&task, "sprites", &slug);
        let warm = assert_warmup(&w, "sprites", &slug, "standard");
        let allow = warm.flag("--sprites-network-allow").expect("allow flag");
        assert!(allow.split(',').any(|d| d == "example.com"), "{allow}");
        assert_eq!(env_of(&warm, "CRABBOX_NAMESPACE_EGRESS_DOMAINS"), None);
        assert_run_targets(&w, "sprites", &slug);
    }
);

parity!(
    start_ssh_sets_static_env_on_warmup_run_and_stop,
    covers = ["B-86"],
    tags = [],
    {
        let (w, repo) = setup(Preset::Ssh, json!({ "ssh_port": 2222 }), false);
        let name = w.task_name("ssh");
        let task = started(&w, &repo, &name);
        let slug = crabbox::slug(&w).expect("warmup slug");
        assert_ready_handle(&task, "ssh", &slug);
        assert_warmup(&w, "ssh", &slug, "standard");
        let m = moor();
        let closed = m.close(&w, &repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        let mut seen = verb_calls(&w, "warmup");
        seen.extend(verb_calls(&w, "run"));
        seen.extend(verb_calls(&w, "stop"));
        assert!(seen.len() >= 3, "warmup, run and stop all recorded");
        for c in &seen {
            assert_eq!(
                env_of(c, "CRABBOX_STATIC_ID").as_deref(),
                Some(slug.as_str())
            );
            assert_eq!(env_of(c, "CRABBOX_STATIC_PORT").as_deref(), Some("2222"));
        }
        let (w2, repo2) = setup(Preset::Ssh, json!({}), false);
        started(&w2, &repo2, &w2.task_name("ssh"));
        let warm = verb_calls(&w2, "warmup").remove(0);
        assert_eq!(env_of(&warm, "CRABBOX_STATIC_PORT").as_deref(), Some("22"));
        let (w3, repo3) = setup(Preset::Ssh, json!({ "ssh_port": 0 }), false);
        let bad = moor().start(
            &w3,
            &repo3.path,
            &w3.task_name("ssh"),
            &StartOpts::default(),
        );
        assert!(bad.is_err(), "ssh_port 0 must be rejected");
        assert!(
            bad.stderr.contains("ssh_port must be 1-65535"),
            "{}",
            bad.stderr
        );
        assert!(verb_calls(&w3, "warmup").is_empty());
    }
);

parity!(
    ssh_box_is_never_inventoried_and_still_stops,
    covers = ["B-87"],
    tags = [],
    {
        let (w, repo) = setup(Preset::Ssh, json!({}), false);
        let name = w.task_name("sshg");
        started(&w, &repo, &name);
        let slug = crabbox::slug(&w).expect("warmup slug");
        crabbox::gone(&w, &slug);
        let m = moor();
        let listed = m.list(&w, &repo.path);
        let tasks = listed.ok();
        let row = tasks.iter().find(|t| t.name == name).expect("listed");
        assert_eq!(row.boxed.as_ref().expect("box").state, BoxState::Ready);
        assert!(verb_calls(&w, "list").is_empty(), "ssh provider is skipped");
        let closed = m.close(&w, &repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        assert!(closed.ok().box_deleted);
        let stops = verb_calls(&w, "stop");
        assert_eq!(stops.len(), 1);
        assert_eq!(stops[0].flag("--provider").as_deref(), Some("ssh"));
        assert_eq!(stops[0].flag("--id").as_deref(), Some(slug.as_str()));
    }
);

parity!(
    close_release_deleted_runs_stop_with_delete_env,
    covers = ["B-77", "B-78", "REQ-11"],
    tags = [],
    {
        let (w, repo) = setup(Preset::LocalContainer, json!({}), false);
        let name = w.task_name("rel");
        started(&w, &repo, &name);
        let slug = crabbox::slug(&w).expect("warmup slug");
        let closed = moor().close(&w, &repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        let view = closed.ok();
        assert!(view.box_deleted);
        assert_eq!(
            view.task.boxed.as_ref().expect("box").state,
            BoxState::Deleted
        );
        let stops = verb_calls(&w, "stop");
        assert_eq!(stops.len(), 1);
        assert_eq!(
            stops[0].flag("--provider").as_deref(),
            Some("local-container")
        );
        assert_eq!(stops[0].flag("--id").as_deref(), Some(slug.as_str()));
        assert_eq!(
            env_of(&stops[0], "CRABBOX_NAMESPACE_DELETE_ON_RELEASE").as_deref(),
            Some("1")
        );
        let kinds: Vec<_> = moor()
            .events(&w, &name)
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert!(kinds.contains(&EventKind::BoxDeleted), "{kinds:?}");
    }
);

parity!(
    close_release_not_found_counts_as_deleted,
    covers = ["B-77", "B-78"],
    tags = [],
    {
        let (w, repo) = setup(Preset::LocalContainer, json!({}), false);
        let name = w.task_name("nf");
        started(&w, &repo, &name);
        crabbox::not_found(&w);
        let closed = moor().close(&w, &repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        let view = closed.ok();
        assert!(view.box_deleted);
        assert_eq!(view.task.state, TaskState::Closed);
        assert_eq!(
            view.task.boxed.as_ref().expect("box").state,
            BoxState::Deleted
        );
        let kinds: Vec<_> = moor()
            .events(&w, &name)
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert!(kinds.contains(&EventKind::BoxDeleted), "{kinds:?}");
    }
);

parity!(
    close_release_failure_on_ready_box_keeps_task_open,
    covers = ["B-77", "B-79"],
    tags = [],
    {
        let (w, repo) = setup(Preset::LocalContainer, json!({}), false);
        let name = w.task_name("rf");
        started(&w, &repo, &name);
        crabbox::fail(&w, "stop");
        let m = moor();
        let closed = m.close(&w, &repo.path, &name, false);
        assert!(closed.is_err(), "close must fail: {}", closed.stderr);
        assert!(
            closed
                .stderr
                .contains("box release failed on provider local-container (exit 1)"),
            "{}",
            closed.stderr
        );
        let listed = m.list(&w, &repo.path);
        let tasks = listed.ok();
        let row = tasks.iter().find(|t| t.name == name).expect("listed");
        assert_ne!(row.state, TaskState::Closed);
    }
);
