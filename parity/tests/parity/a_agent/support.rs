use std::time::{Duration, Instant};

use parity::adapter::{BoxState, EventKind, ExitClass, Moor, TaskState, TaskView};
use parity::fake::crabbox::{self, Preset};
use parity::fake::{claude, herdr, Call};
use parity::world::config::{RepoConfig, UserConfig};
use parity::world::hooks::HookState;
use parity::world::repo::Repo;
use parity::world::World;

use crate::{moor, world};

pub(super) struct Fx {
    pub(super) w: World,
    pub(super) repo: Repo,
    pub(super) m: Box<dyn Moor>,
    pub(super) name: String,
}

pub(super) fn fixture(preset: Preset, location: &str, max_boxes: Option<u32>) -> Fx {
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

pub(super) fn box_fx() -> Fx {
    fixture(Preset::NamespaceDevbox, "box", None)
}

pub(super) fn box_fx_on(preset: Preset) -> Fx {
    fixture(preset, "box", None)
}

pub(super) fn box_fx_max(n: u32) -> Fx {
    fixture(Preset::NamespaceDevbox, "box", Some(n))
}

impl Fx {
    pub(super) fn start_named(&self, name: &str, goal: bool) -> TaskView {
        let opts = parity::adapter::StartOpts {
            goal: goal.then(|| "do the work".to_string()),
            ..Default::default()
        };
        let o = self.m.start(&self.w, &self.repo.path, name, &opts);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        o.ok().clone()
    }

    pub(super) fn start(&self, goal: bool) -> TaskView {
        self.start_named(&self.name, goal)
    }

    pub(super) fn slug(&self) -> String {
        crabbox::slug(&self.w).expect("warmup slug")
    }

    pub(super) fn present(&self) {
        crabbox::inventory(&self.w, &[&self.slug()]);
    }

    pub(super) fn row(&self) -> TaskView {
        let o = self.m.list(&self.w, &self.repo.path);
        o.ok()
            .iter()
            .find(|t| t.name == self.name)
            .expect("task listed")
            .clone()
    }

    pub(super) fn kinds(&self) -> Vec<EventKind> {
        self.m
            .events(&self.w, &self.name)
            .into_iter()
            .map(|e| e.kind)
            .collect()
    }

    pub(super) fn box_state(&self, t: &TaskView) -> BoxState {
        t.boxed.as_ref().expect("box").state.clone()
    }

    pub(super) fn verb(&self, verb: &str) -> Vec<Call> {
        self.w
            .calls("crabbox")
            .into_iter()
            .filter(|c| c.argv.first().map(String::as_str) == Some(verb))
            .collect()
    }

    pub(super) fn put_to_sleep(&self) {
        self.start(true);
        self.present();
        self.w.hook_state(HookState::Idle, 0);
        assert_eq!(self.row().state, TaskState::Idle);
        self.m.age(&self.w, &self.name, 120);
    }

    pub(super) fn asleep(&self) {
        self.put_to_sleep();
        let t = self.row();
        assert_eq!(t.state, TaskState::Asleep);
        assert_eq!(self.box_state(&t), BoxState::Asleep);
    }

    pub(super) fn assert_woken(&self) {
        self.w.hook_state(HookState::Working, 0);
        let t = self.row();
        assert_eq!(t.state, TaskState::Working);
        assert_eq!(self.box_state(&t), BoxState::Ready);
        assert!(self.kinds().contains(&EventKind::Woke));
    }

    pub(super) fn pane_agent(&self) -> parity::adapter::AgentView {
        let o = self.m.pane(&self.w, &self.repo.path, &self.name);
        o.ok().agent.clone().expect("pane agent")
    }

    pub(super) fn wait_agent(&self, timeout: u64) -> parity::adapter::AgentView {
        let o = self
            .m
            .wait(&self.w, &self.repo.path, &self.name, Some(timeout));
        o.ok().agent.clone().expect("wait agent")
    }
}

pub(super) fn hook_reads(w: &World) -> usize {
    w.calls("ssh")
        .iter()
        .filter(|c| c.argv.last().is_some_and(|a| a.contains("agent-state")))
        .count()
}

pub(super) fn timed_out(fx: &Fx, secs: u64) -> parity::adapter::WaitView {
    let o = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, Some(secs));
    assert_eq!(o.class, ExitClass::Failed, "{}", o.stdout);
    let v = o.value.clone().expect("wait json");
    assert!(v.timed_out, "{}", o.stdout);
    v
}

pub(super) fn states(a: &parity::adapter::AgentView) -> (Option<&str>, Option<&str>) {
    (a.agent_state.as_deref(), a.source.as_deref())
}

pub(super) fn user_edit(w: &World, f: impl FnOnce(&mut serde_json::Value)) {
    let raw = std::fs::read_to_string(w.config_path()).expect("read user config");
    let mut doc: serde_json::Value = serde_json::from_str(&raw).expect("parse user config");
    f(&mut doc);
    std::fs::write(w.config_path(), doc.to_string()).expect("write user config");
}

pub(super) fn wait_for_call(w: &World, tool: &str, needle: &str) {
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

pub(super) fn probes(w: &World) -> usize {
    w.calls("crabbox")
        .iter()
        .filter(|c| c.argv.iter().any(|a| a.contains("MOOR_WORKDIR")))
        .count()
}

pub(super) fn timed_out_or_done(fx: &Fx) -> parity::adapter::WaitView {
    let o = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, Some(2));
    o.value.clone().expect("wait json")
}
