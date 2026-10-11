use parity::adapter::{BoxState, EventKind, ExitClass, StartOpts, TaskState, TaskView};
use parity::fake::crabbox::{self, Preset};
use parity::fake::{herdr, Call};
use parity::parity;
use parity::world::repo::Repo;
use parity::world::World;
use serde_json::{json, Value};

use crate::{moor, world};

fn write_user(w: &World, provider: &str, crabbox_keys: Value, enforce_egress: bool) {
    let mut b = json!({ "provider": provider });
    if enforce_egress {
        b["enforce_egress"] = json!(true);
    }
    let doc = json!({ "box": b, "drivers": { "crabbox": crabbox_keys } });
    std::fs::write(w.config_path(), doc.to_string()).expect("write user config");
}

fn write_repo(repo: &Repo, doc: Value) {
    std::fs::write(repo.path.join(".moor.yaml"), doc.to_string()).expect("write repo config");
}

fn verb_calls(w: &World, verb: &str) -> Vec<Call> {
    w.calls("crabbox")
        .into_iter()
        .filter(|c| c.argv.first().map(String::as_str) == Some(verb))
        .collect()
}

fn env_of(c: &Call, key: &str) -> Option<String> {
    c.env.get(key).cloned()
}

fn setup(preset: Preset, crabbox_keys: Value, enforce: bool) -> (World, Repo) {
    let w = world();
    let repo = Repo::create(&w, "widget");
    write_user(&w, preset.provider(), crabbox_keys, enforce);
    crabbox::install_preset(&w, preset);
    herdr::install(&w);
    (w, repo)
}

fn started(w: &World, repo: &Repo, name: &str) -> TaskView {
    let m = moor();
    let o = m.start(w, &repo.path, name, &StartOpts::default());
    assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
    o.ok().clone()
}

fn assert_ready_handle(task: &TaskView, provider: &str, slug: &str) {
    assert_eq!(task.state, TaskState::Working);
    let b = task.boxed.as_ref().expect("box present");
    assert_eq!(b.state, BoxState::Ready);
    assert_eq!(
        b.handle.as_deref(),
        Some(format!("{provider}/{slug}").as_str())
    );
}

fn assert_warmup(w: &World, provider: &str, slug: &str, class: &str) -> Call {
    let warm = verb_calls(w, "warmup");
    assert_eq!(warm.len(), 1, "one warmup");
    let c = warm[0].clone();
    assert_eq!(c.flag("--provider").as_deref(), Some(provider));
    assert_eq!(c.flag("--slug").as_deref(), Some(slug));
    assert_eq!(c.flag("--class").as_deref(), Some(class));
    c
}

fn assert_run_targets(w: &World, provider: &str, slug: &str) {
    let runs = verb_calls(w, "run");
    for r in &runs {
        assert_eq!(r.flag("--provider").as_deref(), Some(provider));
        assert_eq!(r.flag("--id").as_deref(), Some(slug));
    }
}

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

fn failed_start(release: Option<&str>) -> (World, Repo, String, Value) {
    let (w, repo) = setup(Preset::LocalContainer, json!({}), false);
    crabbox::fail_quota(&w);
    match release {
        Some("not_found") => crabbox::not_found(&w),
        Some("error") => crabbox::fail(&w, "stop"),
        _ => {}
    }
    let name = w.task_name("af");
    let m = moor();
    let o = m.start(&w, &repo.path, &name, &StartOpts::default());
    assert_eq!(o.class, ExitClass::Failed, "{}", o.stderr);
    assert_eq!(o.exit, 1);
    let out: Value = serde_json::from_str(&o.stdout).expect("start stdout is JSON");
    assert_eq!(out["task"]["state"], "setup_failed", "{out}");
    let ev = m
        .events(&w, &name)
        .into_iter()
        .find(|e| e.kind == EventKind::BoxFailed)
        .expect("box_failed event")
        .payload;
    (w, repo, name, json!({ "out": out, "event": ev }))
}

fn assert_acquire_failure(w: &World, v: &Value) {
    let slug = crabbox::slug(w).expect("warmup slug");
    let handle = format!("local-container/{slug}");
    let ev = &v["event"];
    let err = ev["error"].as_str().unwrap_or_default();
    assert!(
        err.contains("box acquire failed on provider local-container (exit 2)"),
        "{ev}"
    );
    assert_eq!(ev["handle"], handle.as_str(), "{ev}");
    assert!(
        ev["stderr_tail"]
            .to_string()
            .contains("quota exceeded for plan"),
        "{ev}"
    );
    let stops = verb_calls(w, "stop");
    assert_eq!(stops.len(), 1, "release attempted once");
    assert_eq!(stops[0].flag("--id").as_deref(), Some(slug.as_str()));
}

parity!(
    acquire_failure_release_deleted_keeps_failed_handle,
    covers = ["B-72", "B-73"],
    tags = [],
    {
        let (w, repo, name, v) = failed_start(None);
        assert_acquire_failure(&w, &v);
        assert!(v["event"]["release"].to_string().contains("deleted"), "{v}");
        let tasks = moor().list(&w, &repo.path);
        let row = tasks
            .ok()
            .iter()
            .find(|t| t.name == name)
            .cloned()
            .expect("listed");
        assert_eq!(row.state, TaskState::SetupFailed);
        let b = row.boxed.expect("box");
        assert_eq!(b.state, BoxState::Failed);
        assert!(b.handle.is_some(), "handle kept");
        assert!(v["out"]["reason"].is_string(), "{v}");
    }
);

parity!(
    acquire_failure_release_not_found_marks_gone,
    covers = [],
    tags = [],
    {
        let (w, repo, name, v) = failed_start(Some("not_found"));
        assert_acquire_failure(&w, &v);
        let tasks = moor().list(&w, &repo.path);
        let row = tasks
            .ok()
            .iter()
            .find(|t| t.name == name)
            .cloned()
            .expect("listed");
        let b = row.boxed.expect("box");
        assert_eq!(b.state, BoxState::Gone);
        assert_eq!(b.handle, None);
    }
);

parity!(
    acquire_failure_release_error_does_not_mask_original,
    covers = [],
    tags = [],
    {
        let (w, repo, name, v) = failed_start(Some("error"));
        assert_acquire_failure(&w, &v);
        let release = v["event"]["release"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(release.starts_with("error:"), "{v}");
        let tasks = moor().list(&w, &repo.path);
        let row = tasks
            .ok()
            .iter()
            .find(|t| t.name == name)
            .cloned()
            .expect("listed");
        let b = row.boxed.expect("box");
        assert_eq!(b.state, BoxState::Failed);
        assert!(b.handle.is_some());
    }
);

parity!(
    close_failed_box_release_error_becomes_release_failed,
    covers = ["B-79"],
    tags = [],
    {
        let (w, repo, name, _) = failed_start(None);
        crabbox::fail(&w, "stop");
        let closed = moor().close(&w, &repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        assert!(
            closed.stderr.contains("not deleted") && closed.stderr.contains("handle kept"),
            "{}",
            closed.stderr
        );
        let view = closed.ok();
        assert_eq!(view.task.state, TaskState::Closed);
        let b = view.task.boxed.as_ref().expect("box");
        assert_eq!(b.state, BoxState::ReleaseFailed);
        assert!(b.handle.is_some());
        let ev = moor()
            .events(&w, &name)
            .into_iter()
            .find(|e| e.kind == EventKind::Other("box_release_failed".into()))
            .expect("box_release_failed event");
        assert!(ev.payload["handle"].is_string(), "{}", ev.payload);
        assert!(ev.payload["error"].is_string(), "{}", ev.payload);
    }
);

fn inventory_shapes(slug: &str) -> Vec<Value> {
    vec![
        json!([{ "name": slug }]),
        json!([{ "slug": slug }]),
        json!([{ "id": slug }]),
        json!([{ "labels": { "slug": slug } }]),
        json!([{ "labels": { "lease": slug } }]),
    ]
}

parity!(
    list_reports_box_gone_when_provider_omits_it,
    covers = ["B-80", "REQ-16"],
    tags = [],
    {
        let (w, repo) = setup(Preset::LocalContainer, json!({}), false);
        let name = w.task_name("gone");
        started(&w, &repo, &name);
        let slug = crabbox::slug(&w).expect("warmup slug");
        let m = moor();
        let state_of = |w: &World| -> BoxState {
            let l = m.list(w, &repo.path);
            let t = l
                .ok()
                .iter()
                .find(|t| t.name == name)
                .cloned()
                .expect("listed");
            t.boxed.expect("box").state
        };
        for shape in inventory_shapes(&slug) {
            w.prepend_rules(
                "crabbox",
                [parity::fake::Rule::on(&["list"]).stdout(shape.to_string())],
            );
            assert_eq!(state_of(&w), BoxState::Ready, "{shape}");
        }
        let before = verb_calls(&w, "list").len();
        assert_eq!(before, 5, "one list per reconcile");
        let last = verb_calls(&w, "list").pop().expect("list call");
        assert_eq!(last.flag("--provider").as_deref(), Some("local-container"));
        assert!(last.argv.iter().any(|a| a == "--json"));
        crabbox::fail(&w, "list");
        assert_eq!(state_of(&w), BoxState::Ready, "list failure leaves box");
        w.prepend_rules(
            "crabbox",
            [parity::fake::Rule::on(&["list"]).stdout("not json")],
        );
        assert_eq!(state_of(&w), BoxState::Ready, "bad output leaves box");
        crabbox::gone(&w, &slug);
        assert_eq!(state_of(&w), BoxState::Gone);
        let kinds: Vec<_> = m.events(&w, &name).into_iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&EventKind::BoxGone), "{kinds:?}");
    }
);

fn warmups(w: &World) -> usize {
    verb_calls(w, "warmup").len()
}

fn find(tasks: &[TaskView], name: &str) -> TaskView {
    tasks
        .iter()
        .find(|t| t.name == name)
        .cloned()
        .expect("task listed")
}

parity!(
    max_boxes_queues_and_close_promotes_oldest,
    covers = ["B-94", "B-95", "B-96"],
    tags = [],
    {
        let (w, repo) = setup(Preset::LocalContainer, json!({ "max_boxes": 1 }), false);
        let (a, b, c) = (w.task_name("qa"), w.task_name("qb"), w.task_name("qc"));
        started(&w, &repo, &a);
        let slug_a = crabbox::slug(&w).expect("warmup slug");
        crabbox::inventory(&w, &[&slug_a]);
        let m = moor();
        for n in [&b, &c] {
            let o = m.start(&w, &repo.path, n, &StartOpts::default());
            assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
            let t = o.ok();
            assert_eq!(t.state, TaskState::WaitingForBox);
            assert!(t.boxed.is_none(), "no Box while waiting");
            assert!(t.worktree.is_some() && t.workspace_id.is_some());
        }
        assert_eq!(warmups(&w), 1, "queued starts do not warm up");
        let ev = m
            .events(&w, &b)
            .into_iter()
            .find(|e| e.kind == EventKind::WaitingForBox)
            .expect("waiting_for_box event");
        assert_eq!(ev.payload["max_boxes"], 1, "{}", ev.payload);
        assert_eq!(ev.payload["driver"], "crabbox", "{}", ev.payload);

        let closed = m.close(&w, &repo.path, &a, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        assert_eq!(warmups(&w), 2, "close promoted exactly one waiter");
        let slug_b = crabbox::slug(&w).expect("promoted slug");
        assert!(slug_b.contains(&b), "oldest waiter promoted: {slug_b}");
        crabbox::inventory(&w, &[&slug_b]);
        let listed = m.list(&w, &repo.path);
        let tasks = listed.ok();
        let tb = find(tasks, &b);
        assert_ne!(tb.state, TaskState::WaitingForBox);
        assert_eq!(tb.boxed.as_ref().expect("box").state, BoxState::Ready);
        assert_eq!(find(tasks, &c).state, TaskState::WaitingForBox);
        assert_eq!(warmups(&w), 2);
    }
);

parity!(
    max_boxes_gone_box_frees_capacity_and_zero_is_rejected,
    covers = [],
    tags = [],
    {
        let (w, repo) = setup(Preset::LocalContainer, json!({ "max_boxes": 1 }), false);
        let (a, b) = (w.task_name("ga"), w.task_name("gb"));
        started(&w, &repo, &a);
        let slug = crabbox::slug(&w).expect("warmup slug");
        crabbox::gone(&w, &slug);
        let m = moor();
        let l = m.list(&w, &repo.path);
        assert_eq!(find(l.ok(), &a).boxed.expect("box").state, BoxState::Gone);
        let t = started(&w, &repo, &b);
        assert_eq!(t.state, TaskState::Working);
        assert_eq!(warmups(&w), 2);

        let (w0, repo0) = setup(Preset::LocalContainer, json!({ "max_boxes": 0 }), false);
        let bad = moor().start(&w0, &repo0.path, &w0.task_name("z"), &StartOpts::default());
        assert!(bad.is_err());
        assert!(
            bad.stderr.contains("max_boxes must be at least 1"),
            "{}",
            bad.stderr
        );
        assert_eq!(warmups(&w0), 0);
    }
);

parity!(
    gone_drops_only_the_named_box_from_inventory,
    covers = [],
    tags = [],
    {
        let (w, repo) = setup(Preset::LocalContainer, json!({}), false);
        let (a, b) = (w.task_name("pa"), w.task_name("pb"));
        started(&w, &repo, &a);
        let slug_a = crabbox::slug(&w).expect("warmup slug");
        started(&w, &repo, &b);
        let slug_b = crabbox::slug(&w).expect("warmup slug");
        crabbox::inventory(&w, &[&slug_a, &slug_b]);
        crabbox::gone(&w, &slug_a);
        let listed = moor().list(&w, &repo.path);
        let tasks = listed.ok();
        assert_eq!(find(tasks, &a).boxed.expect("box").state, BoxState::Gone);
        assert_eq!(find(tasks, &b).boxed.expect("box").state, BoxState::Ready);
    }
);
