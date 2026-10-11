use std::time::{Duration, Instant};

use parity::adapter::{BoxState, EventKind, ExitClass, Moor, TaskState, TaskView};
use parity::assert::{assert_called, assert_never_called, assert_order};
use parity::fake::crabbox::{self, Preset};
use parity::fake::{claude, herdr, Call};
use parity::parity;
use parity::world::config::{RepoConfig, UserConfig};
use parity::world::hooks::HookState;
use parity::world::repo::Repo;
use parity::world::World;

use crate::{moor, world};

struct Fx {
    w: World,
    repo: Repo,
    m: Box<dyn Moor>,
    name: String,
}

fn fixture(preset: Preset, location: &str, max_boxes: Option<u32>) -> Fx {
    let w = world();
    let repo = Repo::create(&w, "widget");
    let mut user = UserConfig::default().provider(preset.provider());
    user.sleep_after_minutes = Some(1);
    user.max_boxes = max_boxes;
    user.write(&w);
    RepoConfig::default()
        .agent_location(location)
        .write(&repo.path);
    crabbox::install_preset(&w, preset);
    crabbox::ssh_channel(&w);
    crabbox::tunnel(&w);
    crabbox::connect(&w);
    herdr::install(&w);
    claude::install(&w);
    let name = w.task_name("agt");
    Fx {
        w,
        repo,
        m: moor(),
        name,
    }
}

fn host_fx(preset: Preset) -> Fx {
    fixture(preset, "host", None)
}

fn box_fx() -> Fx {
    fixture(Preset::NamespaceDevbox, "box", None)
}

fn agents(w: &World, state: &str) {
    let doc = format!(r#"[{{"id":"a1","sessionId":"a1-full","cwd":"","state":"{state}"}}]"#);
    claude::agents(w, &doc);
}

impl Fx {
    fn start_named(&self, name: &str, goal: bool) -> TaskView {
        let opts = parity::adapter::StartOpts {
            goal: goal.then(|| "do the work".to_string()),
            ..Default::default()
        };
        let o = self.m.start(&self.w, &self.repo.path, name, &opts);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        o.ok().clone()
    }

    fn start(&self, goal: bool) -> TaskView {
        self.start_named(&self.name, goal)
    }

    fn slug(&self) -> String {
        crabbox::slug(&self.w).expect("warmup slug")
    }

    fn present(&self) {
        crabbox::inventory(&self.w, &[&self.slug()]);
    }

    fn row(&self) -> TaskView {
        let o = self.m.list(&self.w, &self.repo.path);
        o.ok()
            .iter()
            .find(|t| t.name == self.name)
            .expect("task listed")
            .clone()
    }

    fn kinds(&self) -> Vec<EventKind> {
        self.m
            .events(&self.w, &self.name)
            .into_iter()
            .map(|e| e.kind)
            .collect()
    }

    fn box_state(&self, t: &TaskView) -> BoxState {
        t.boxed.as_ref().expect("box").state.clone()
    }

    fn verb(&self, verb: &str) -> Vec<Call> {
        self.w
            .calls("crabbox")
            .into_iter()
            .filter(|c| c.argv.first().map(String::as_str) == Some(verb))
            .collect()
    }

    fn put_to_sleep(&self) {
        self.start(true);
        self.present();
        agents(&self.w, "done");
        assert_eq!(self.row().state, TaskState::Idle);
        self.m.age(&self.w, &self.name, 120);
    }

    fn asleep(&self) {
        self.put_to_sleep();
        let t = self.row();
        assert_eq!(t.state, TaskState::Asleep);
        assert_eq!(self.box_state(&t), BoxState::Asleep);
    }

    fn assert_woken(&self) {
        agents(&self.w, "working");
        let t = self.row();
        assert_eq!(t.state, TaskState::Working);
        assert_eq!(self.box_state(&t), BoxState::Ready);
        assert!(self.kinds().contains(&EventKind::Woke));
    }

    fn pane_agent(&self) -> parity::adapter::AgentView {
        let o = self.m.pane(&self.w, &self.repo.path, &self.name);
        o.ok().agent.clone().expect("pane agent")
    }

    fn wait_agent(&self, timeout: u64) -> parity::adapter::AgentView {
        let o = self
            .m
            .wait(&self.w, &self.repo.path, &self.name, Some(timeout));
        o.ok().agent.clone().expect("wait agent")
    }
}

parity!(
    host_agent_done_idles_and_busy_returns,
    covers = ["A-60", "A-61", "B-104"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        assert_eq!(fx.row().state, TaskState::Working);
        agents(&fx.w, "done");
        assert_eq!(fx.row().state, TaskState::Idle);
        assert!(fx.kinds().contains(&EventKind::Idled));
        agents(&fx.w, "working");
        assert_eq!(fx.row().state, TaskState::Working);
        assert!(fx.kinds().contains(&EventKind::Busy));
    }
);

parity!(
    start_pass_evaluates_other_tasks,
    covers = ["A-60", "B-82"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(false);
        assert!(!fx.kinds().contains(&EventKind::Idled));
        fx.start_named(&fx.w.task_name("second"), false);
        assert!(fx.kinds().contains(&EventKind::Idled));
    }
);

parity!(
    pane_and_wait_do_not_evaluate_idle,
    covers = ["A-60", "A-75"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        agents(&fx.w, "done");
        let p = fx.m.pane(&fx.w, &fx.repo.path, &fx.name);
        assert_eq!(p.ok().task.state, TaskState::Working);
        let wt = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, Some(5));
        assert_eq!(wt.class, ExitClass::Ok, "{}", wt.stderr);
        assert!(!fx.kinds().contains(&EventKind::Idled));
        assert_eq!(fx.row().state, TaskState::Idle);
    }
);

parity!(
    idle_task_sleeps_via_pause_after_age,
    covers = ["A-62", "B-103", "B-105"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        agents(&fx.w, "done");
        assert_eq!(fx.row().state, TaskState::Idle);
        assert_eq!(fx.row().state, TaskState::Idle);
        assert!(fx.verb("pause").is_empty(), "no pause before the idle age");
        fx.m.age(&fx.w, &fx.name, 120);
        let t = fx.row();
        assert_eq!(t.state, TaskState::Asleep);
        assert_eq!(fx.box_state(&t), BoxState::Asleep);
        let pauses = fx.verb("pause");
        assert_eq!(pauses.len(), 1);
        assert_eq!(
            pauses[0].flag("--provider").as_deref(),
            Some("namespace-devbox")
        );
        assert_eq!(pauses[0].flag("--id"), Some(fx.slug()));
        assert_eq!(fx.row().state, TaskState::Asleep);
        assert_eq!(fx.verb("pause").len(), 1);
    }
);

parity!(
    unsupported_provider_stays_up_and_reports_reason,
    covers = ["A-62", "A-70", "B-105"],
    tags = [],
    {
        let fx = host_fx(Preset::LocalContainer);
        fx.start(true);
        fx.present();
        agents(&fx.w, "done");
        assert_eq!(fx.row().state, TaskState::Idle);
        fx.m.age(&fx.w, &fx.name, 120);
        let t = fx.row();
        assert_eq!(t.state, TaskState::Idle);
        assert_eq!(fx.box_state(&t), BoxState::Ready);
        assert!(t.sleep_reason().is_some_and(|r| !r.is_empty()));
        assert!(fx.verb("pause").is_empty());
        assert!(fx.row().sleep_reason().is_some());
    }
);

parity!(
    sprites_sleep_and_wake_make_no_driver_call,
    covers = ["A-62", "A-63", "B-89"],
    tags = [],
    {
        let fx = host_fx(Preset::Sprites);
        fx.asleep();
        assert!(fx.verb("pause").is_empty());
        let o = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "continue");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(fx.verb("resume").is_empty());
        fx.assert_woken();
    }
);

parity!(
    steer_wakes_asleep_task_with_resume,
    covers = ["A-63", "A-80", "B-106"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.asleep();
        let o = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "continue");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let resumes = fx.verb("resume");
        assert_eq!(resumes.len(), 1);
        assert_eq!(
            resumes[0].flag("--provider").as_deref(),
            Some("namespace-devbox")
        );
        assert_eq!(resumes[0].flag("--id"), Some(fx.slug()));
        fx.assert_woken();
        let calls = fx.w.all_calls();
        assert_order(
            &calls,
            &[
                ("crabbox", &["resume"]),
                ("claude", &["--resume", "a1-full"]),
            ],
        );
    }
);

parity!(
    open_wakes_asleep_task_and_reports_woke,
    covers = ["A-63", "A-76", "B-106", "B-107"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.asleep();
        let o = fx.m.open(&fx.w, &fx.repo.path, &fx.name);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(o.ok().woke);
        assert_eq!(fx.verb("resume").len(), 1);
        assert_called(&fx.w.calls("herdr"), "herdr", &["workspace", "focus"]);
        fx.assert_woken();
        let again = fx.m.open(&fx.w, &fx.repo.path, &fx.name);
        assert!(!again.ok().woke);
        assert_eq!(fx.verb("resume").len(), 1);
        let missing = fx.m.open(&fx.w, &fx.repo.path, "no-such-task");
        assert!(missing.is_err());
    }
);

parity!(
    run_on_idle_task_marks_busy_by_run,
    covers = ["B-19"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        agents(&fx.w, "done");
        assert_eq!(fx.row().state, TaskState::Idle);
        let o = fx.m.run(
            &fx.w,
            &fx.repo.path,
            &fx.name,
            &["true"],
            &Default::default(),
        );
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let busy: Vec<_> =
            fx.m.events(&fx.w, &fx.name)
                .into_iter()
                .filter(|e| e.kind == EventKind::Busy)
                .collect();
        assert_eq!(busy.len(), 1, "{busy:?}");
        assert_eq!(busy[0].payload["by"], "run");
        assert!(fx.verb("resume").is_empty());
    }
);

parity!(
    run_and_cp_wake_asleep_box,
    covers = ["A-63", "A-88", "A-89", "B-19", "B-106"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.asleep();
        let o = fx.m.run(
            &fx.w,
            &fx.repo.path,
            &fx.name,
            &["exit 3"],
            &Default::default(),
        );
        assert_eq!(o.class, ExitClass::Failed);
        assert_eq!(o.exit, 3);
        assert_eq!(fx.verb("resume").len(), 1);
        fx.assert_woken();
        let calls = fx.w.all_calls();
        assert_order(&calls, &[("crabbox", &["resume"]), ("crabbox", &["run"])]);
        assert!(fx.kinds().contains(&EventKind::Ran));

        let fx = host_fx(Preset::NamespaceDevbox);
        fx.asleep();
        let dest = fx.w.root().join("copied.txt");
        let o =
            fx.m.cp(&fx.w, &fx.repo.path, &fx.name, "/tmp/out.txt", &dest);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(fx.verb("resume").len(), 1);
        assert_order(
            &fx.w.all_calls(),
            &[("crabbox", &["resume"]), ("crabbox", &["cp"])],
        );
        fx.assert_woken();
        assert!(fx.kinds().contains(&EventKind::Copied));
    }
);

parity!(
    failed_resume_keeps_task_asleep,
    covers = ["A-63", "B-106"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.asleep();
        crabbox::fail(&fx.w, "resume");
        let o = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "continue");
        assert!(o.is_err());
        let t = fx.row();
        assert_eq!(t.state, TaskState::Asleep);
        assert_eq!(fx.box_state(&t), BoxState::Asleep);
        assert_never_called(&fx.w.calls("claude"), "claude", &["--resume"]);
    }
);

parity!(
    gone_box_is_reconciled_and_never_woken,
    covers = ["A-64", "B-81"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.asleep();
        let t = fx.row();
        assert_eq!(fx.box_state(&t), BoxState::Asleep);
        crabbox::fail(&fx.w, "list");
        assert_eq!(fx.box_state(&fx.row()), BoxState::Asleep);
        crabbox::inventory(&fx.w, &["somebody-else"]);
        let t = fx.row();
        assert_eq!(fx.box_state(&t), BoxState::Gone);
        assert!(fx.kinds().contains(&EventKind::BoxGone));
        let run = fx.m.run(
            &fx.w,
            &fx.repo.path,
            &fx.name,
            &["true"],
            &Default::default(),
        );
        assert!(run.is_err());
        let cp = fx.m.cp(
            &fx.w,
            &fx.repo.path,
            &fx.name,
            "/tmp/x",
            &fx.w.root().join("x"),
        );
        assert!(cp.is_err());
        let sh = fx.m.shell(&fx.w, &fx.repo.path, &fx.name);
        assert!(sh.is_err());
        assert!(fx.verb("resume").is_empty());
        assert!(fx.verb("connect").is_empty());
    }
);

parity!(
    asleep_box_counts_toward_capacity,
    covers = ["B-107"],
    tags = [],
    {
        let fx = fixture(Preset::NamespaceDevbox, "host", Some(1));
        fx.asleep();
        let other = fx.start_named(&fx.w.task_name("queued"), false);
        assert_eq!(other.state, TaskState::WaitingForBox);
    }
);

parity!(
    shell_connects_to_ready_box_only,
    covers = ["A-77", "B-93"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        std::fs::write(fx.repo.path.join("crabbox.yaml"), "provider: x\n").unwrap();
        let task = fx.start(false);
        fx.present();
        let o = fx.m.shell(&fx.w, &fx.repo.path, &fx.name);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let connects = fx.verb("connect");
        assert_eq!(connects.len(), 1);
        assert_eq!(
            connects[0].flag("--provider").as_deref(),
            Some("namespace-devbox")
        );
        assert_eq!(connects[0].flag("--id"), Some(fx.slug()));
        assert!(connects[0].argv.iter().any(|a| a == "--reclaim"));
        assert_eq!(
            std::fs::canonicalize(&connects[0].cwd).ok(),
            task.worktree
                .as_deref()
                .and_then(|p| std::fs::canonicalize(p).ok())
        );
        let cfg = connects[0]
            .env
            .get("CRABBOX_CONFIG")
            .expect("CRABBOX_CONFIG");
        assert!(cfg.ends_with("crabbox.yaml"), "{cfg}");
        crabbox::deny(&fx.w, "connect", 7);
        let failing = fx.m.shell(&fx.w, &fx.repo.path, &fx.name);
        assert_eq!(failing.exit, 7);
        let missing = fx.m.shell(&fx.w, &fx.repo.path, "no-such-task");
        assert!(missing.is_err());
    }
);

parity!(
    explicit_sleep_and_wake_commands,
    covers = ["REQ-09"],
    tags = ["new_only(ExplicitSleep)"],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        let slept = fx.m.sleep_cmd(&fx.w, &fx.repo.path, &fx.name);
        assert_eq!(slept.ok().state, TaskState::Asleep);
        assert_eq!(fx.verb("pause").len(), 1);
        let woke = fx.m.wake_cmd(&fx.w, &fx.repo.path, &fx.name);
        assert_eq!(woke.ok().state, TaskState::Working);
        assert_eq!(fx.verb("resume").len(), 1);
    }
);

fn hook_reads(w: &World) -> usize {
    w.calls("ssh")
        .iter()
        .filter(|c| c.argv.last().is_some_and(|a| a.contains("agent-state")))
        .count()
}

fn agents_with(w: &World, state: &str, extra: &str) {
    let doc = format!(r#"[{{"id":"a1","sessionId":"a1-full","cwd":"","state":"{state}"{extra}}}]"#);
    claude::agents(w, &doc);
}

fn timed_out(fx: &Fx, secs: u64) -> parity::adapter::WaitView {
    let o = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, Some(secs));
    assert_eq!(o.class, ExitClass::Failed, "{}", o.stdout);
    let v = o.value.clone().expect("wait json");
    assert!(v.timed_out, "{}", o.stdout);
    v
}

fn states(a: &parity::adapter::AgentView) -> (Option<&str>, Option<&str>) {
    (a.agent_state.as_deref(), a.source.as_deref())
}

parity!(
    wait_idle_task_without_agent_returns,
    covers = ["A-85", "A-86"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(false);
        fx.present();
        let o = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, None);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let v = o.ok();
        assert!(!v.timed_out);
        assert!(v.run.is_none());
    }
);

parity!(
    wait_host_agent_blocks_until_done,
    covers = ["A-85", "A-86"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        let v = timed_out(&fx, 1);
        assert_eq!(v.agent.expect("agent").state, "working");
        agents(&fx.w, "done");
        let o = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, Some(5));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().agent.as_ref().expect("agent").state, "done");
    }
);

parity!(
    wait_blocks_while_box_hook_says_working,
    covers = ["A-73", "A-74", "A-85", "A-86", "A-87", "REQ-02"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Working, 0);
        let began = Instant::now();
        let v = timed_out(&fx, 1);
        assert!(
            began.elapsed() < Duration::from_secs(6),
            "{:?}",
            began.elapsed()
        );
        let a = v.agent.expect("agent");
        assert_eq!(states(&a), (Some("working"), Some("hooks")));
    }
);

parity!(
    wait_returns_when_box_hook_says_idle,
    covers = ["A-73", "A-74", "A-85", "REQ-03", "REQ-05"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Idle, 0);
        let o = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, Some(5));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let a = o.ok().agent.clone().expect("agent");
        assert_eq!(states(&a), (Some("idle"), Some("hooks")));
        assert_eq!(a.state, "live");
    }
);

parity!(
    wait_background_marker_overrides_idle_hook,
    covers = ["A-74", "REQ-05"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Idle, 0);
        herdr::pane_text(&fx.w, "  2 background agents running\n> \n");
        let v = timed_out(&fx, 1);
        assert_eq!(
            states(&v.agent.expect("agent")),
            (Some("working"), Some("pane"))
        );
    }
);

parity!(
    wait_ends_when_box_agent_is_blocked,
    covers = ["A-74", "A-85"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Blocked, 0);
        let o = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, Some(5));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let a = o.ok().agent.clone().expect("agent");
        assert_eq!(states(&a), (Some("blocked"), Some("hooks")));
    }
);

parity!(
    box_agent_without_hook_falls_back_to_herdr_then_pane,
    covers = ["A-74", "REQ-05"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        let cases: [(Option<&str>, &str, (&str, &str)); 5] = [
            (Some("working"), "", ("working", "herdr")),
            (Some("blocked"), "", ("blocked", "herdr")),
            (Some("idle"), "all done\n", ("idle", "herdr")),
            (
                Some("idle"),
                "Cooked for 4s \u{b7} done\n> \n",
                ("idle", "pane"),
            ),
            (None, "thinking (esc to interrupt)\n", ("working", "pane")),
        ];
        for (status, text, want) in cases {
            herdr::pane_list(&fx.w, status);
            herdr::pane_text(&fx.w, text);
            let a = fx.pane_agent();
            assert_eq!(
                states(&a),
                (Some(want.0), Some(want.1)),
                "{status:?} {text:?}"
            );
        }
    }
);

parity!(
    exited_marker_ends_wait_and_reports_code,
    covers = ["A-71", "A-72", "A-75", "A-85", "A-86"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        let live = fx.pane_agent();
        assert_eq!(live.state, "live");
        assert_eq!(live.exit_code, None);
        herdr::pane_text(&fx.w, "bye\nmoor: agent exited 3\n");
        let o = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, Some(5));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let a = o.ok().agent.clone().expect("agent");
        assert_eq!(a.state, "exited");
        assert_eq!(a.exit_code, Some(3));
        let p = fx.pane_agent();
        assert_eq!((p.state.as_str(), p.exit_code), ("exited", Some(3)));
        assert_eq!(fx.row().agent().expect("row agent").state, "exited");
    }
);

parity!(
    gone_pane_reports_gone_agent,
    covers = ["A-71", "A-72"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        herdr::pane_gone(&fx.w);
        assert_eq!(fx.pane_agent().state, "gone");
        let o = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, Some(5));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().agent.as_ref().expect("agent").state, "gone");
    }
);

parity!(
    failed_hook_read_uses_cache_only_while_fresh,
    covers = ["A-73", "REQ-03"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Idle, 0);
        assert_eq!(states(&fx.wait_agent(5)), (Some("idle"), Some("hooks")));
        fx.w.clear_hook_state();
        herdr::pane_list(&fx.w, Some("working"));
        assert_eq!(
            states(&fx.pane_agent()),
            (Some("idle"), Some("hooks-cached"))
        );
        fx.m.age(&fx.w, &fx.name, 200);
        assert_eq!(states(&fx.pane_agent()), (Some("working"), Some("herdr")));
    }
);

parity!(
    hook_read_cut_at_deadline_is_unknown_never_idle,
    covers = ["A-73", "A-87", "REQ-02", "REQ-03"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Idle, 0);
        let hang = || {
            fx.w.prepend_rules(
                "ssh",
                [parity::fake::Rule::any()
                    .contains("agent-state")
                    .block()
                    .times(1)],
            )
        };
        hang();
        let began = Instant::now();
        let v = timed_out(&fx, 1);
        assert!(
            began.elapsed() < Duration::from_secs(6),
            "{:?}",
            began.elapsed()
        );
        assert_eq!(
            states(&v.agent.expect("agent")),
            (Some("unknown"), Some("timeout"))
        );
        assert_eq!(states(&fx.wait_agent(5)), (Some("idle"), Some("hooks")));
        hang();
        assert_eq!(
            states(&fx.wait_agent(5)),
            (Some("idle"), Some("hooks-cached"))
        );
    }
);

parity!(
    starting_hook_reads_working_only_while_young,
    covers = ["A-73", "REQ-07"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Starting, 0);
        herdr::pane_list(&fx.w, Some("idle"));
        let v = timed_out(&fx, 1);
        assert_eq!(
            states(&v.agent.expect("agent")),
            (Some("working"), Some("hooks"))
        );
        fx.m.age(&fx.w, &fx.name, 200);
        assert_eq!(states(&fx.wait_agent(5)), (Some("idle"), Some("herdr")));
    }
);

parity!(
    pane_wait_and_list_agree_on_agent_state,
    covers = ["A-70", "A-71", "REQ-04"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        for (hook, want) in [
            (HookState::Idle, ("idle", "hooks")),
            (HookState::Working, ("working", "hooks")),
            (HookState::Blocked, ("blocked", "hooks")),
        ] {
            fx.w.hook_state(hook, 0);
            let pane = fx.pane_agent();
            let list = fx.row().agent().expect("row agent");
            let wait = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, Some(1));
            let wait = wait
                .value
                .clone()
                .and_then(|v| v.agent)
                .expect("wait agent");
            for a in [&pane, &list, &wait] {
                assert_eq!(states(a), (Some(want.0), Some(want.1)));
            }
        }
    }
);

parity!(
    one_hook_read_per_call,
    covers = ["REQ-04"],
    tags = ["new_only(SingleRead)"],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Idle, 0);
        let before = hook_reads(&fx.w);
        fx.row();
        assert_eq!(
            hook_reads(&fx.w) - before,
            1,
            "list reads the hook file once"
        );
        let before = hook_reads(&fx.w);
        fx.pane_agent();
        assert_eq!(
            hook_reads(&fx.w) - before,
            1,
            "pane reads the hook file once"
        );
        let before = hook_reads(&fx.w);
        fx.wait_agent(5);
        assert_eq!(
            hook_reads(&fx.w) - before,
            1,
            "wait reads the hook file once"
        );
    }
);

parity!(
    box_launch_wires_claude_hooks_to_state_file,
    covers = ["REQ-05", "REQ-06"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        let run =
            fx.w.calls("herdr")
                .into_iter()
                .find(|c| c.argv.starts_with(&["pane".to_string(), "run".to_string()]))
                .expect("agent pane run");
        let script = run.argv[3].replace(['\\', '\''], "");
        for pair in [
            r#""state":"working","event":"UserPromptSubmit""#,
            r#""state":"idle","event":"Stop""#,
            r#""state":"waiting_input","event":"Notification""#,
            r#""state":"waiting_input","event":"PreToolUse""#,
            r#""state":"working","event":"PostToolUse""#,
            r#""state":"starting""#,
        ] {
            assert!(script.contains(pair), "launch lacks {pair}");
        }
        for matcher in [
            "permission_prompt",
            "elicitation_dialog",
            "AskUserQuestion",
            "ExitPlanMode",
        ] {
            assert!(script.contains(matcher), "launch lacks matcher {matcher}");
        }
        assert!(
            !script.contains("idle_prompt"),
            "idle notification must not block"
        );
    }
);

parity!(
    steer_host_resumes_the_same_session,
    covers = ["A-80", "A-81"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        let o = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "keep going");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let resume =
            fx.w.calls("claude")
                .into_iter()
                .find(|c| c.argv.first().map(String::as_str) == Some("--resume"))
                .expect("claude --resume");
        assert_eq!(resume.argv[1], "a1-full");
        assert!(resume.argv.iter().any(|a| a == "--bg"));
        assert_eq!(resume.argv.last().map(String::as_str), Some("keep going"));
        assert_never_called(&fx.w.calls("claude"), "claude", &["stop"]);
    }
);

parity!(
    steer_host_stops_a_live_session_before_resuming,
    covers = ["A-81"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        fx.w.prepend_rules(
            "claude",
            [parity::fake::Rule::on(&["agents"])
                .stdout(
                    r#"[{"id":"a1","sessionId":"a1-full","cwd":"","state":"working","pid":4242}]"#,
                )
                .times(2)],
        );
        let o = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "keep going");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_order(
            &fx.w.all_calls(),
            &[
                ("claude", &["stop", "a1"]),
                ("claude", &["--resume", "a1-full"]),
            ],
        );
    }
);

parity!(
    steer_host_refuses_a_blocked_agent,
    covers = ["A-81"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        agents_with(&fx.w, "blocked", r#","waitingFor":"approval""#);
        let o = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "keep going");
        assert!(o.is_err());
        let calls = fx.w.calls("claude");
        assert_never_called(&calls, "claude", &["--resume"]);
        assert_never_called(&calls, "claude", &["stop"]);
        let a = fx.row().agent().expect("agent");
        assert_eq!(a.state, "blocked");
        assert_eq!(a.waiting_for.as_deref(), Some("approval"));
    }
);

parity!(
    steer_box_pane_types_message_and_submits,
    covers = ["A-80", "A-82"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        let o = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "do more");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_order(
            &fx.w.all_calls(),
            &[
                ("herdr", &["pane", "send-keys", "p-1", "ctrl+u"]),
                ("herdr", &["pane", "send-text", "p-1", "do more"]),
                ("herdr", &["pane", "send-keys", "p-1", "ctrl+m"]),
            ],
        );
        assert_never_called(&fx.w.calls("claude"), "claude", &["--resume"]);
    }
);

parity!(
    steer_box_pane_refuses_gone_or_exited_pane,
    covers = ["A-82"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        herdr::pane_text(&fx.w, "bye\nmoor: agent exited 3\n");
        let exited = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "again");
        assert!(exited.is_err());
        herdr::pane_gone(&fx.w);
        let gone = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "again");
        assert!(gone.is_err());
        assert_never_called(&fx.w.calls("herdr"), "herdr", &["pane", "send-text"]);
    }
);

parity!(
    steer_needs_an_open_task_and_a_live_session,
    covers = ["A-80"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(false);
        fx.present();
        let none = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "hello");
        assert!(none.is_err());
        let unknown = fx.m.steer(&fx.w, &fx.repo.path, "no-such-task", "hello");
        assert!(unknown.is_err());
        assert_never_called(&fx.w.calls("claude"), "claude", &["--resume"]);
    }
);

parity!(
    host_agent_state_reaches_list_row,
    covers = ["A-70", "A-71"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        for state in ["working", "done", "failed", "stopped"] {
            agents(&fx.w, state);
            let a = fx.row().agent().expect("agent");
            assert_eq!(a.state, state);
            assert_eq!(a.waiting_for, None);
        }
        agents_with(&fx.w, "blocked", r#","waitingFor":"approval""#);
        let a = fx.row().agent().expect("agent");
        assert_eq!(a.state, "blocked");
        assert_eq!(a.waiting_for.as_deref(), Some("approval"));
    }
);

fn user_edit(w: &World, f: impl FnOnce(&mut serde_json::Value)) {
    let raw = std::fs::read_to_string(w.config_path()).expect("read user config");
    let mut doc: serde_json::Value = serde_json::from_str(&raw).expect("parse user config");
    f(&mut doc);
    std::fs::write(w.config_path(), doc.to_string()).expect("write user config");
}

fn wait_for_call(w: &World, tool: &str, needle: &str) {
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(10) {
        let seen = w
            .calls(tool)
            .iter()
            .any(|c| c.argv.iter().any(|a| a.contains(needle)));
        if seen {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("no {tool} call containing {needle}");
}

fn probes(w: &World) -> usize {
    w.calls("crabbox")
        .iter()
        .filter(|c| c.argv.iter().any(|a| a.contains("MOOR_WORKDIR")))
        .count()
}

parity!(
    rerun_start_reacquires_a_gone_box,
    covers = ["B-75"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(false);
        let first = fx.slug();
        crabbox::inventory(&fx.w, &["somebody-else"]);
        let gone = fx.row();
        assert_eq!(fx.box_state(&gone), BoxState::Gone);
        let again = fx.start(false);
        assert_eq!(fx.verb("warmup").len(), 2);
        assert_eq!(fx.box_state(&again), BoxState::Ready);
        assert_ne!(fx.slug(), first);
        assert_eq!(
            again
                .boxed
                .and_then(|b| b.handle)
                .as_deref()
                .map(|h| h.ends_with(&fx.slug())),
            Some(true)
        );
    }
);

parity!(
    waiting_task_refuses_run_cp_and_shell,
    covers = ["B-97"],
    tags = [],
    {
        let fx = fixture(Preset::NamespaceDevbox, "host", Some(1));
        fx.start(false);
        fx.present();
        let queued = fx.w.task_name("queued");
        let t = fx.start_named(&queued, false);
        assert_eq!(t.state, TaskState::WaitingForBox);
        let dest = fx.w.root().join("never");
        assert!(fx
            .m
            .run(
                &fx.w,
                &fx.repo.path,
                &queued,
                &["true"],
                &Default::default()
            )
            .is_err());
        assert!(fx
            .m
            .cp(&fx.w, &fx.repo.path, &queued, "/tmp/x", &dest)
            .is_err());
        assert!(fx.m.shell(&fx.w, &fx.repo.path, &queued).is_err());
        assert!(fx.verb("cp").is_empty());
        assert!(fx.verb("connect").is_empty());
        assert!(fx
            .verb("run")
            .iter()
            .all(|c| !c.argv.iter().any(|a| a == "true")));
    }
);

parity!(
    repo_layer_box_provider_is_rejected,
    covers = ["B-85"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        std::fs::write(
            fx.repo.path.join(".moor.yaml"),
            r#"{"box":{"provider":"sprites"}}"#,
        )
        .unwrap();
        let opts = parity::adapter::StartOpts::default();
        let o = fx.m.start(&fx.w, &fx.repo.path, &fx.name, &opts);
        assert!(o.is_err(), "{}", o.stdout);
        assert!(fx.verb("warmup").is_empty());
    }
);

parity!(
    namespace_start_warns_unless_egress_is_enforced,
    covers = ["B-88"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        let o =
            fx.m.start(&fx.w, &fx.repo.path, &fx.name, &Default::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(!o.warnings.is_empty(), "namespace start must warn");
        let warm = &fx.verb("warmup")[0];
        assert!(!warm.env.contains_key("CRABBOX_NAMESPACE_EGRESS_DOMAINS"));

        let other = host_fx(Preset::LocalContainer);
        let o = other
            .m
            .start(&other.w, &other.repo.path, &other.name, &Default::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(o.warnings.is_empty(), "{:?}", o.warnings);
    }
);

parity!(
    unknown_provider_takes_the_generic_path,
    covers = ["B-91"],
    tags = [],
    {
        let fx = host_fx(Preset::LocalContainer);
        user_edit(&fx.w, |d| d["box"]["provider"] = "acme-cloud".into());
        let task = fx.start(true);
        fx.present();
        assert_eq!(task.state, TaskState::Working);
        let slug = fx.slug();
        let w1 = fx.verb("warmup");
        assert_eq!(w1[0].flag("--provider").as_deref(), Some("acme-cloud"));
        let ran = fx.m.run(
            &fx.w,
            &fx.repo.path,
            &fx.name,
            &["true"],
            &Default::default(),
        );
        assert_eq!(ran.class, ExitClass::Ok, "{}", ran.stderr);
        let runs = fx.verb("run");
        assert_eq!(runs[0].flag("--provider").as_deref(), Some("acme-cloud"));
        assert_eq!(runs[0].flag("--id"), Some(slug.clone()));
        agents(&fx.w, "done");
        assert_eq!(fx.row().state, TaskState::Idle);
        fx.m.age(&fx.w, &fx.name, 120);
        let t = fx.row();
        assert_eq!(t.state, TaskState::Idle);
        assert!(t.sleep_reason().is_some());
        assert!(fx.verb("pause").is_empty());
        let closed = fx.m.close(&fx.w, &fx.repo.path, &fx.name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        let stop = &fx.verb("stop")[0];
        assert_eq!(stop.flag("--provider").as_deref(), Some("acme-cloud"));
        assert_eq!(stop.flag("--id"), Some(slug));

        let bad = host_fx(Preset::LocalContainer);
        user_edit(&bad.w, |d| d["box"]["provider"] = "acme-cloud".into());
        crabbox::fail_quota(&bad.w);
        let o = bad
            .m
            .start(&bad.w, &bad.repo.path, &bad.name, &Default::default());
        assert_eq!(o.class, ExitClass::Failed);
    }
);

parity!(
    box_state_reads_go_over_the_crabbox_ssh_channel,
    covers = ["B-112", "B-113"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Idle, 0);
        assert_eq!(states(&fx.wait_agent(5)), (Some("idle"), Some("hooks")));
        let ssh = fx.verb("ssh");
        assert!(!ssh.is_empty());
        assert_eq!(
            ssh[0].flag("--provider").as_deref(),
            Some("namespace-devbox")
        );
        assert_eq!(ssh[0].flag("--id"), Some(fx.slug()));
        let read =
            fx.w.calls("ssh")
                .into_iter()
                .rfind(|c| c.argv.last().is_some_and(|a| a.contains("agent-state")))
                .expect("hook read over ssh");
        assert_eq!(&read.argv[..3], ["-o", "BatchMode=yes", "box"]);
        let script = read.argv.last().unwrap();
        assert!(script.contains(&fx.w.hook_state_path().display().to_string()));
        assert!(
            script.contains("44"),
            "absent file maps to a distinct exit: {script}"
        );

        fx.w.clear_hook_state();
        herdr::pane_list(&fx.w, Some("working"));
        fx.m.age(&fx.w, &fx.name, 200);
        assert_eq!(states(&fx.pane_agent()), (Some("working"), Some("herdr")));

        crabbox::deny(&fx.w, "ssh", 1);
        let a = fx.pane_agent();
        assert_eq!(states(&a), (Some("working"), Some("herdr")));
    }
);

parity!(
    workdir_is_resolved_once_and_again_after_wake,
    covers = ["B-114"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Idle, 0);
        let first = probes(&fx.w);
        assert_eq!(first, 1);
        fx.wait_agent(5);
        fx.pane_agent();
        fx.wait_agent(5);
        assert_eq!(probes(&fx.w), first, "reads reuse the resolved workdir");
        assert_eq!(fx.row().state, TaskState::Idle);
        fx.m.age(&fx.w, &fx.name, 120);
        assert_eq!(fx.row().state, TaskState::Asleep);
        let o = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "wake up");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        fx.wait_agent(5);
        assert!(probes(&fx.w) > first, "wake drops the cached workdir");
    }
);

parity!(
    reads_stay_bounded_while_a_run_holds_the_box,
    covers = ["REQ-01", "REQ-08"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        let done = fx.m.run(
            &fx.w,
            &fx.repo.path,
            &fx.name,
            &["echo first-run"],
            &Default::default(),
        );
        assert_eq!(done.class, ExitClass::Ok, "{}", done.stderr);
        fx.w.prepend_rules(
            "crabbox",
            [parity::fake::Rule::on(&["run"])
                .contains("hold-run")
                .block()],
        );
        let mut run = fx.m.run_bg(&fx.w, &fx.repo.path, &fx.name, "hold-run");
        wait_for_call(&fx.w, "crabbox", "hold-run");
        let began = Instant::now();
        assert_eq!(fx.row().state, TaskState::Working);
        let p = fx.m.pane(&fx.w, &fx.repo.path, &fx.name);
        assert_eq!(p.class, ExitClass::Ok, "{}", p.stderr);
        let v = timed_out(&fx, 1);
        assert!(
            began.elapsed() < Duration::from_secs(8),
            "{:?}",
            began.elapsed()
        );
        let current = v.run.expect("in-flight run");
        assert!(current.to_string().contains("hold-run"), "{current}");
        assert_eq!(current["in_progress"], true, "{current}");
        run.kill().expect("kill run");
        run.wait().expect("reap run");
        let v = timed_out_or_done(&fx);
        let after = v.run.expect("interrupted run");
        assert!(after.to_string().contains("hold-run"), "{after}");
        assert_eq!(after["interrupted"], true, "{after}");
    }
);

fn timed_out_or_done(fx: &Fx) -> parity::adapter::WaitView {
    let o = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, Some(2));
    o.value.clone().expect("wait json")
}

parity!(
    dead_provisioning_refuses_run_and_restart,
    covers = ["B-76"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.w.prepend_rules("crabbox", [parity::fake::Rule::on(&["warmup"]).block()]);
        let mut start =
            fx.m.start_detached(&fx.w, &fx.repo.path, &fx.name, &Default::default());
        wait_for_call(&fx.w, "crabbox", "warmup");
        start.kill().expect("kill start");
        start.wait().expect("reap start");
        fx.m.age(&fx.w, &fx.name, 300);
        let warmups = fx.verb("warmup").len();
        let again =
            fx.m.start(&fx.w, &fx.repo.path, &fx.name, &Default::default());
        assert!(again.is_err(), "{}", again.stdout);
        let run = fx.m.run(
            &fx.w,
            &fx.repo.path,
            &fx.name,
            &["true"],
            &Default::default(),
        );
        assert!(run.is_err());
        assert_eq!(fx.verb("warmup").len(), warmups);
    }
);

parity!(
    dotfiles_dir_is_uploaded_and_installed,
    covers = ["B-115"],
    tags = [],
    {
        let fx = box_fx();
        let dots = fx.w.root().join("dots");
        std::fs::create_dir_all(&dots).unwrap();
        std::fs::write(dots.join(".bashrc"), "# dots\n").unwrap();
        std::os::unix::fs::symlink(".bashrc", dots.join("linked")).unwrap();
        let install = "#!/bin/sh\ntouch \"$HOME/dotfiles-ran\"\n";
        std::fs::write(dots.join("install.sh"), install).unwrap();
        let repo_cfg = r#"{"setup":["echo hi"],"agent":{"location":"box"}}"#;
        std::fs::write(fx.repo.path.join(".moor.yaml"), repo_cfg).unwrap();
        user_edit(&fx.w, |d| {
            d["dotfiles"] = serde_json::json!({ "dir": dots.display().to_string() })
        });
        let o =
            fx.m.start(&fx.w, &fx.repo.path, &fx.name, &Default::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let home = fx.w.box_root().join("home");
        assert!(
            home.join("dotfiles-ran").exists(),
            "install.sh ran in the Box"
        );
        let uploaded = std::fs::read_dir(&home)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.join(".bashrc").exists())
            .expect("dotfiles directory in the Box home");
        let linked = std::fs::symlink_metadata(uploaded.join("linked")).unwrap();
        assert!(
            !linked.file_type().is_symlink(),
            "symlinks are dereferenced"
        );
        assert_order(
            &fx.w.all_calls(),
            &[("ssh", &["-o"]), ("crabbox", &["run"])],
        );
    }
);
