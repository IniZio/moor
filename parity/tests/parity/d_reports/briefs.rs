use super::*;

fn report_command(brief: &str, name: &str) -> String {
    brief
        .split('`')
        .skip(1)
        .step_by(2)
        .find(|seg| seg.contains(&format!(" report {name} --kind ")))
        .unwrap_or_else(|| panic!("brief names no report command for {name}: {brief}"))
        .to_string()
}

parity!(
    local_brief_gives_the_agent_a_working_report_command,
    covers = ["D-26", "D-31"],
    tags = [],
    {
        let f = fx();
        let name = f.name("brief");
        let t = f.start_with(&name, Some("write a spec"));
        let call = f.bg_call();
        let brief = call.argv[0].clone();
        assert!(brief.contains(".moor/reports/"), "{brief}");
        assert!(!brief.contains("publish_artifact"), "{brief}");
        let argv = call.argv.join(" ");
        assert!(
            argv.contains(&format!("report {name}")),
            "report is allowed"
        );
        assert!(!argv.contains("mcp__agentic-artifacts"), "{argv}");

        let wt = PathBuf::from(t.worktree.expect("worktree"));
        std::fs::create_dir_all(wt.join(".moor/reports")).expect("mkdir");
        std::fs::write(wt.join(".moor/reports/spec.md"), "agent wrote this").expect("report");
        let cmd = report_command(&brief, &name)
            .replace("<kind>", "spec")
            .replace("<short title>", "Agent title");
        let out =
            f.w.command("sh")
                .args(["-c", &cmd])
                .current_dir(&wt)
                .env_remove("MOOR_STATE_DIR")
                .env_remove("MOOR_CONFIG")
                .output()
                .expect("run report command");
        assert!(
            out.status.success(),
            "{cmd}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let listed = f.reports(&name);
        let rows = listed.ok();
        assert_eq!(rows.len(), 1, "the agent's command reached this state dir");
        assert_eq!(rows[0].kind, "spec");
        assert_eq!(rows[0].title.as_deref(), Some("Agent title"));
        let copy = rows[0].copy_path().expect("file url");
        f.assert_stored_under_state(&copy);
        assert_eq!(read(&copy), "agent wrote this");
    }
);

parity!(
    artifacts_brief_publishes_then_reports_the_link,
    covers = ["D-27"],
    tags = [],
    {
        let doc = json!({ "reports": { "backend": "agentic-artifacts" } });
        let f = fx_with_config(Some(doc));
        let name = f.name("art");
        f.start_with(&name, Some("write a spec"));
        let call = f.bg_call();
        let brief = &call.argv[0];
        for needle in [
            "publish_artifact",
            "commit_artifact",
            "sha256sum",
            "--link",
            "--version",
            "widget",
            &format!("{name}/"),
        ] {
            assert!(brief.contains(needle), "{needle} missing: {brief}");
        }
        assert!(
            brief.contains("--file"),
            "the local fallback stays available"
        );
        let argv = call.argv.join(" ");
        for tool in [
            "mcp__agentic-artifacts__publish_artifact",
            "mcp__agentic-artifacts__commit_artifact",
        ] {
            assert!(argv.contains(tool), "{tool} not allowed: {argv}");
        }
        assert!(argv.contains(&format!("report {name}")));
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
        f.w.prepend_rules(
            "claude",
            [Rule::on(&["stop"]).exit(1).stderr("no such session\n")],
        );
        claude::agents(&f.w, "[]");
        crabbox::gone(&f.w, &crabbox::slug(&f.w).expect("slug"));
        let o = f.m.close(&f.w, &f.repo.path, &name, false);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().task.state, TaskState::Closed);
        assert_eq!(f.row(&name).state, TaskState::Closed);
    }
);
