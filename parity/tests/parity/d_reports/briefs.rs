use super::*;

parity!(
    state_dir_flag_pins_registry,
    covers = ["D-31"],
    tags = [],
    {
        let f = fx();
        let name = f.start("pinned");
        let src = f.src("pin.md", "pinned");
        let o = f.report(&name, &ReportReq::file("spec", &src));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let listed = f.reports(&name);
        let rows = listed.ok();
        assert_eq!(rows.len(), 1, "the report reached this state dir");
        let copy = rows[0].copy_path().expect("file url");
        f.assert_stored_under_state(&copy);
        assert_eq!(read(&copy), "pinned");
    }
);

parity!(
    box_report_reaches_the_registry_through_cp_then_report_file,
    covers = ["D-29"],
    tags = [],
    {
        let f = fx();
        let name = f.start("boxrep");
        let in_box = f.w.box_root().join("work/.moor/reports");
        std::fs::create_dir_all(&in_box).expect("box reports dir");
        std::fs::write(in_box.join("spec.md"), "from the box").expect("box report");
        let dest = f.w.tmp_dir().join("fetched.md");
        let cp =
            f.m.cp(&f.w, &f.repo.path, &name, ".moor/reports/spec.md", &dest);
        assert_eq!(cp.class, ExitClass::Ok, "{}", cp.stderr);
        assert_eq!(read(&dest), "from the box");
        let row = f.report(&name, &ReportReq::file("spec", &dest));
        let row = row.ok().clone();
        assert_eq!((row.kind.as_str(), row.version), ("spec", 1));
        assert_eq!(read(&row.copy_path().expect("file url")), "from the box");
    }
);

fn failed_acquire(f: &Fx, name: &str) -> TaskView {
    crabbox::fail_quota(&f.w);
    let o = f.m.start(&f.w, &f.repo.path, name, &StartOpts::default());
    assert!(o.is_err());
    f.row(name)
}

parity!(
    failed_acquire_surfaces_the_driver_cause,
    covers = ["REQ-17"],
    tags = [],
    {
        let f = fx();
        let name = f.name("cause");
        crabbox::fail_quota(&f.w);
        let o = f.m.start(&f.w, &f.repo.path, &name, &StartOpts::default());
        assert!(o.is_err());
        let said = format!("{}{}", o.stdout, o.stderr);
        assert!(said.contains("quota exceeded"), "cause lost: {said}");
        assert_eq!(f.row(&name).state, TaskState::SetupFailed);
    }
);

parity!(
    close_of_unreleased_box_warns_in_moor_words,
    covers = ["REQ-10"],
    tags = ["new_only(MoorWarning)"],
    {
        let f = fx();
        let name = f.name("unrel");
        failed_acquire(&f, &name);
        f.w.prepend_rules(
            "crabbox",
            [Rule::on(&["stop"]).exit(1).stderr("RAWDRIVER-7731\n")],
        );
        let o = f.m.close(&f.w, &f.repo.path, &name, false);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let view = o.ok();
        assert!(!view.box_deleted);
        assert_eq!(view.task.state, TaskState::Closed);
        let b = view.task.boxed.as_ref().expect("box");
        assert_eq!(b.state, BoxState::ReleaseFailed);
        assert!(
            b.handle.is_some(),
            "the handle is kept for a manual release"
        );
        assert!(
            !o.warnings.is_empty(),
            "close warns about the unreleased Box"
        );
        assert!(
            o.warnings.iter().all(|w| !w.contains("RAWDRIVER-7731")),
            "{:?}",
            o.warnings
        );
    }
);

parity!(
    cp_success_prints_a_confirmation,
    covers = ["REQ-24"],
    tags = ["new_only(CpConfirms)"],
    {
        let f = fx();
        let name = f.start("cpmsg");
        std::fs::write(f.w.box_root().join("work/out.txt"), "x").expect("box file");
        let dest = f.w.tmp_dir().join("out.txt");
        let o = f.m.cp(&f.w, &f.repo.path, &name, "out.txt", &dest);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(
            !o.stdout.trim().is_empty(),
            "cp prints something on success"
        );
    }
);

parity!(
    version_output_identifies_the_commit,
    covers = ["REQ-46"],
    tags = ["new_only(VersionHash)"],
    {
        let f = fx();
        let o = f.m.version(&f.w);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let has_hash = o
            .ok()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|t| t.len() >= 7 && t.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(has_hash, "no commit hash in {:?}", o.ok());
    }
);

parity!(
    close_tolerates_missing_worktree_workspace_and_agent,
    covers = ["REQ-12"],
    tags = [],
    {
        let f = fx();
        let name = f.name("miss");
        let t = f.start_with(&name, Some("go"));
        let wt = PathBuf::from(t.worktree.clone().expect("worktree"));
        let o =
            f.w.command("git")
                .current_dir(&f.repo.path)
                .args(["worktree", "remove", "--force"])
                .arg(&wt)
                .output()
                .expect("git worktree remove");
        assert!(o.status.success());
        f.w.prepend_rules(
            "herdr",
            [Rule::on(&["workspace", "close"])
                .exit(1)
                .stderr(r#"{"error":{"code":"workspace_not_found","message":"gone"}}"#)],
        );
        herdr::pane_gone(&f.w);
        crabbox::gone(&f.w, &crabbox::slug(&f.w).expect("slug"));
        let o = f.m.close(&f.w, &f.repo.path, &name, false);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().task.state, TaskState::Closed);
        assert_eq!(f.row(&name).state, TaskState::Closed);
    }
);
