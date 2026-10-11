use std::path::{Path, PathBuf};

use parity::adapter::{
    BoxState, CloseView, ExitClass, Moor, Outcome, ReportReq, ReportRow, StartOpts, TaskState,
    TaskView,
};
use parity::fake::{claude, crabbox, herdr, Call, Rule};
use parity::parity;
use parity::world::config::UserConfig;
use parity::world::repo::Repo;
use parity::world::World;
use serde_json::{json, Value};

use crate::{moor, world};

struct Fx {
    w: World,
    repo: Repo,
    m: Box<dyn Moor>,
}

fn fx_with_config(config: Option<Value>) -> Fx {
    let w = world();
    let repo = Repo::create(&w, "widget");
    match config {
        Some(doc) => std::fs::write(w.config_path(), doc.to_string()).expect("write config"),
        None => UserConfig::default().write(&w),
    }
    crabbox::install(&w);
    crabbox::cp(&w);
    herdr::install(&w);
    claude::install(&w);
    Fx { w, repo, m: moor() }
}

fn fx() -> Fx {
    fx_with_config(None)
}

impl Fx {
    fn name(&self, base: &str) -> String {
        self.w.task_name(base)
    }

    fn start_with(&self, name: &str, goal: Option<&str>) -> TaskView {
        let opts = StartOpts {
            goal: goal.map(str::to_string),
            ..Default::default()
        };
        let o = self.m.start(&self.w, &self.repo.path, name, &opts);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        if let Some(slug) = crabbox::slug(&self.w) {
            crabbox::inventory(&self.w, &[&slug]);
        }
        o.ok().clone()
    }

    fn start(&self, base: &str) -> String {
        let name = self.name(base);
        self.start_with(&name, None);
        name
    }

    fn src(&self, file: &str, body: &str) -> PathBuf {
        let p = self.w.tmp_dir().join(file);
        std::fs::write(&p, body).expect("write source");
        p
    }

    fn report(&self, name: &str, req: &ReportReq) -> Outcome<ReportRow> {
        self.m.report(&self.w, &self.repo.path, name, req)
    }

    fn reports(&self, name: &str) -> Outcome<Vec<ReportRow>> {
        self.m.reports(&self.w, &self.repo.path, name)
    }

    fn row(&self, name: &str) -> TaskView {
        let o = self.m.list(&self.w, &self.repo.path);
        o.ok()
            .iter()
            .rfind(|t| t.name == name)
            .expect("task listed")
            .clone()
    }

    fn close(&self, name: &str) -> CloseView {
        let o = self.m.close(&self.w, &self.repo.path, name, false);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        o.ok().clone()
    }

    fn stored_files(&self) -> Vec<PathBuf> {
        files_under(&self.w.state_dir().join("reports"))
    }

    fn assert_stored_under_state(&self, copy: &Path) {
        let root = std::fs::canonicalize(self.w.state_dir().join("reports")).expect("reports dir");
        let real = std::fs::canonicalize(copy).expect("copy exists");
        assert!(
            real.starts_with(&root),
            "{} not under state",
            real.display()
        );
    }

    fn bg_call(&self) -> Call {
        self.w
            .calls("claude")
            .into_iter()
            .find(|c| c.argv.iter().any(|a| a == "--bg"))
            .expect("claude --bg dispatched")
    }
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

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
