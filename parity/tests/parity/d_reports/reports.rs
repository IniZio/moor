use super::*;

parity!(
    report_file_copies_and_versions_per_kind,
    covers = ["D-01", "D-02", "D-03", "D-11"],
    tags = [],
    {
        let f = fx();
        let name = f.start("rep");
        let src = f.src("a.md", "first");
        let v1 = f.report(&name, &ReportReq::file("spec", &src).title(""));
        assert_eq!(v1.class, ExitClass::Ok, "{}", v1.stderr);
        let v1 = v1.ok().clone();
        assert_eq!((v1.kind.as_str(), v1.version), ("spec", 1));
        assert_eq!(v1.backend, "local");
        assert_eq!(v1.title, None, "empty title is stored as absent");
        let copy1 = v1.copy_path().expect("local link is a file url");
        f.assert_stored_under_state(&copy1);
        assert_eq!(copy1.extension().and_then(|e| e.to_str()), Some("md"));
        assert_eq!(read(&copy1), "first");

        std::fs::write(&src, "second").expect("edit source");
        let v2 = f.report(&name, &ReportReq::file("spec", &src).title("Second"));
        let v2 = v2.ok().clone();
        assert_eq!(v2.version, 2);
        assert_eq!(v2.title.as_deref(), Some("Second"));
        let copy2 = v2.copy_path().expect("file url");
        assert_ne!(copy1, copy2);
        assert_eq!(read(&copy1), "first", "an earlier version never changes");
        assert_eq!(read(&copy2), "second");

        let bare = f.src("result", "done");
        let other = f.report(&name, &ReportReq::file("result", &bare));
        let other = other.ok().clone();
        assert_eq!((other.kind.as_str(), other.version), ("result", 1));
        assert_eq!(other.copy_path().expect("file url").extension(), None);

        let listed = f.reports(&name);
        let listed = listed.ok();
        let shape: Vec<(&str, i64)> = listed
            .iter()
            .map(|r| (r.kind.as_str(), r.version))
            .collect();
        assert_eq!(shape, [("spec", 1), ("spec", 2), ("result", 1)]);
    }
);

parity!(
    report_link_uses_artifacts_backend_with_explicit_versions,
    covers = ["D-04", "D-05", "D-06"],
    tags = [],
    {
        let f = fx();
        let name = f.start("lnk");
        let url = "https://artifacts.example/v/abc?x=1";
        let first = f.report(&name, &ReportReq::link("doc", url));
        let first = first.ok().clone();
        assert_eq!(first.backend, "agentic-artifacts");
        assert_eq!((first.link.as_str(), first.version), (url, 1));
        assert!(f.stored_files().is_empty(), "a link report stores no copy");

        let third = f.report(&name, &ReportReq::link("doc", url).version(3));
        assert_eq!(third.ok().version, 3);
        let next = f.report(&name, &ReportReq::link("doc", url));
        assert_eq!(next.ok().version, 4, "next version follows the maximum");

        let dup = f.report(&name, &ReportReq::link("doc", url).version(3));
        assert!(dup.is_err(), "a duplicate version is refused");
        let versions: Vec<i64> = f.reports(&name).ok().iter().map(|r| r.version).collect();
        assert_eq!(versions, [1, 3, 4], "the refused duplicate records nothing");
    }
);

parity!(
    report_rejects_bad_input_and_records_nothing,
    covers = ["D-07", "D-08", "D-09", "D-10", "D-12", "D-13"],
    tags = [],
    {
        let f = fx();
        let name = f.start("bad");
        let src = f.src("ok.md", "body");
        let too_long = "k".repeat(65);
        let failed = [
            ReportReq::file("a/b", &src),
            ReportReq::file("../up", &src),
            ReportReq::file("", &src),
            ReportReq::file(&too_long, &src),
            ReportReq::link("doc", "u").version(0),
            ReportReq::link("doc", "   "),
            ReportReq::file("spec", f.w.tmp_dir().join("missing.md")),
        ];
        for req in &failed {
            let o = f.report(&name, req);
            assert_eq!(o.class, ExitClass::Failed, "{req:?}: {}", o.stdout);
        }
        let both = ReportReq {
            link: Some("u".into()),
            ..ReportReq::file("spec", &src)
        };
        let neither = ReportReq {
            kind: "spec".into(),
            ..Default::default()
        };
        let version_with_file = ReportReq {
            version: Some(2),
            ..ReportReq::file("spec", &src)
        };
        for req in [both, neither, version_with_file] {
            assert_eq!(f.report(&name, &req).class, ExitClass::Usage, "{req:?}");
        }
        let unknown = f.report("no-such-task", &ReportReq::file("spec", &src));
        assert_eq!(unknown.class, ExitClass::Failed);
        assert!(f.reports(&name).ok().is_empty());
        assert!(f.stored_files().is_empty(), "no stray copies");

        let longest = "k".repeat(64);
        let edge = f.report(&name, &ReportReq::file(&longest, &src));
        assert_eq!(edge.class, ExitClass::Ok, "{}", edge.stderr);

        f.close(&name);
        let closed = f.report(&name, &ReportReq::file("spec", &src));
        assert_eq!(
            closed.class,
            ExitClass::Failed,
            "closed Task takes no Report"
        );
        assert_eq!(f.reports(&name).ok().len(), 1);
    }
);

parity!(
    reports_stay_readable_and_stored_after_close_and_remove,
    covers = ["D-19", "D-24"],
    tags = [],
    {
        let f = fx();
        let name = f.start("keep");
        let src = f.src("r.md", "kept");
        let a = f.report(&name, &ReportReq::file("spec", &src)).ok().clone();
        f.report(&name, &ReportReq::link("doc", "https://x/y"));
        f.close(&name);
        let after = f.reports(&name);
        assert_eq!(after.class, ExitClass::Ok, "{}", after.stderr);
        assert_eq!(after.ok().len(), 2);
        let copy = a.copy_path().expect("file url");
        assert_eq!(read(&copy), "kept");

        let removed = f.m.remove(&f.w, &f.repo.path, &name, false);
        assert_eq!(removed.class, ExitClass::Ok, "{}", removed.stderr);
        assert_eq!(read(&copy), "kept", "remove leaves stored Reports alone");

        let unknown = f.reports("no-such-task");
        assert_eq!(unknown.class, ExitClass::Failed);
    }
);

parity!(
    reporting_never_changes_state_or_gates_close,
    covers = ["D-17", "REQ-49"],
    tags = [],
    {
        let f = fx();
        let name = f.name("calm");
        f.start_with(&name, Some("write a plan"));
        let before = f.row(&name);
        let src = f.src("plan.md", "plan");
        let o = f.report(&name, &ReportReq::file("plan", &src));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let after = f.row(&name);
        assert_eq!(before.state, TaskState::Working);
        assert_eq!(after.state, before.state);
        assert_eq!(
            after.agent().map(|a| a.state),
            before.agent().map(|a| a.state)
        );
        assert_eq!(after.boxed.map(|b| b.state), before.boxed.map(|b| b.state));

        let closed = f.close(&name);
        assert_eq!(closed.task.state, TaskState::Closed);
        assert!(closed.box_deleted);
        let bare = f.start("bare");
        let closed = f.close(&bare);
        assert_eq!(closed.task.state, TaskState::Closed, "no Report needed");
    }
);

parity!(
    list_and_pane_rows_carry_reports,
    covers = ["D-21", "D-22"],
    tags = [],
    {
        let f = fx();
        let name = f.start("rows");
        assert!(f.row(&name).reports().is_empty());
        let src = f.src("s.md", "s");
        let first = f.report(&name, &ReportReq::file("spec", &src)).ok().clone();
        let second = f.report(&name, &ReportReq::link("doc", "https://x/y"));
        let second = second.ok().clone();
        let expect = vec![first, second];
        assert_eq!(f.row(&name).reports(), expect);
        let pane = f.m.pane(&f.w, &f.repo.path, &name);
        assert_eq!(pane.class, ExitClass::Ok, "{}", pane.stderr);
        assert_eq!(pane.ok().task.reports(), expect);
    }
);

parity!(
    pane_picks_the_live_task_among_same_name_rows,
    covers = ["D-23"],
    tags = [],
    {
        let f = fx();
        let name = f.name("dup");
        let cfg = || {
            std::fs::write(
                f.repo.path.join(".moor.yaml"),
                json!({ "credentials": ["NOPE"] }).to_string(),
            )
            .expect("write repo config");
        };
        cfg();
        for _ in 0..2 {
            let o = f.m.start(&f.w, &f.repo.path, &name, &StartOpts::default());
            assert!(o.is_err());
        }
        let ids: Vec<Option<i64>> =
            f.m.list(&f.w, &f.repo.path)
                .ok()
                .iter()
                .filter(|t| t.name == name)
                .map(|t| t.id)
                .collect();
        assert_eq!(ids.len(), 2);
        let pane = f.m.pane(&f.w, &f.repo.path, &name);
        assert_eq!(pane.ok().task.id, ids[1], "all rejected: the latest row");

        std::fs::remove_file(f.repo.path.join(".moor.yaml")).expect("remove config");
        let live = f.start_with(&name, None);
        assert_eq!(live.state, TaskState::Working);
        let src = f.src("d.md", "d");
        f.report(&name, &ReportReq::file("spec", &src));
        let pane = f.m.pane(&f.w, &f.repo.path, &name);
        assert_eq!(
            pane.ok().task.id,
            live.id,
            "the live Task, not a rejected one"
        );
        assert_eq!(pane.ok().task.state, TaskState::Working);
        assert_eq!(pane.ok().task.reports().len(), 1);
        let listed = f.reports(&name);
        assert_eq!(listed.ok().len(), 1, "reports follow the live Task");
    }
);

parity!(
    reports_backend_config_accepts_known_values_only,
    covers = ["D-25"],
    tags = [],
    {
        let f = fx();
        let check = |doc: Value| {
            std::fs::write(f.w.config_path(), doc.to_string()).expect("write config");
            f.m.env(&f.w, &f.repo.path, &Default::default()).class
        };
        assert_eq!(check(json!({})), ExitClass::Ok);
        assert_eq!(
            check(json!({"reports": {"backend": "local"}})),
            ExitClass::Ok
        );
        let artifacts = json!({"reports": {"backend": "agentic-artifacts"}});
        assert_eq!(check(artifacts), ExitClass::Ok);
        let bad_backend = check(json!({"reports": {"backend": "s3"}}));
        assert_ne!(bad_backend, ExitClass::Ok, "unknown backend");
        let bad_key = check(json!({"reports": {"nope": 1}}));
        assert_ne!(bad_key, ExitClass::Ok, "unknown key under reports");
    }
);
