use super::*;

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
