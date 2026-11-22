//! A Ready Box with gates, a live agent and a fake clock, and the gate pass run over it.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::box_state::{BoxLifecycleInput, SetupOutcome};
use moor_core::domain::decision::{Decision, Field, HandsFields, NewBox, SpecFields};
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::domain::session_state::SessionInput;
use moor_core::gate::{CheckDoc, CheckOf, GateDoc, GateLine, GateStatus, GatesDoc, OutcomeDoc};
use moor_core::progress::Progress;
use moor_core::request::StepReportReq;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError};
use moor_core::seam::engine::Steer;
use moor_core::seam::proc::Tool;
use moor_core::view::workflow::StepReportValue;
use moor_core::workflow::{StepId, StepReport, StepResult};
use moor_core::{BoxId, BoxName, BoxPath, ProviderId, ShowReq, SteerReq};

use crate::cx::Cx;
use crate::ops::gate::checks::{pass, run, Job};
use crate::ops::workflow::step_report;
use crate::store::StoreError;
use crate::testing::agent;
use crate::testing::cx::REQUESTER_USER;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::{Reply, ScriptedProc};

pub(super) struct Fx {
    pub(super) dir: tempfile::TempDir,
    pub(super) cx: Cx,
    pub(super) driver: Arc<FakeDriver>,
    pub(super) engine: Arc<FakeEngine>,
    pub(super) id: BoxId,
}

/// A gate whose checks are `checks`, tried up to `tries` times.
pub(super) fn gate(name: &str, checks: &[&str], tries: u64) -> GateDoc {
    GateDoc {
        name: name.to_owned(),
        evidence: Vec::new(),
        checks: checks.iter().map(|c| CheckDoc::from(*c)).collect(),
        approval: None,
        max_tries: Some(tries),
    }
}

/// A gate that asks for `evidence` and then runs `checks`, tried up to `tries` times.
pub(super) fn evidence_gate(name: &str, evidence: &[&str], checks: &[&str], tries: u64) -> GateDoc {
    GateDoc {
        evidence: evidence.iter().map(|e| (*e).to_owned()).collect(),
        ..gate(name, checks, tries)
    }
}

/// Where a Box with no outcomes stands once every gate passed: at the implicit `done`.
pub(super) fn done() -> GateStatus {
    GateStatus::Passed {
        outcome: serde_json::from_value(serde_json::json!({"name": "done", "tone": "good"}))
            .unwrap(),
    }
}

/// A Box `t` with `gates` and an agent that works, over a driver that probes it clean.
pub(super) fn fx(gates: impl FnOnce(&std::path::Path) -> Vec<GateDoc>) -> Fx {
    fx_on(FakeDriver::new(), gates)
}

/// [`fx`] over `driver`.
pub(super) fn fx_on(
    driver: FakeDriver,
    gates: impl FnOnce(&std::path::Path) -> Vec<GateDoc>,
) -> Fx {
    fx_doc(driver, |dir| GatesDoc {
        gates: gates(dir),
        outcomes: Vec::new(),
    })
}

/// [`fx`] with the gates and outcomes `doc` gives.
pub(super) fn fx_outcomes(
    doc: impl FnOnce(&std::path::Path) -> (Vec<GateDoc>, Vec<OutcomeDoc>),
) -> Fx {
    fx_outcomes_on(FakeDriver::new(), doc)
}

/// [`fx_outcomes`] over `driver`.
pub(super) fn fx_outcomes_on(
    driver: FakeDriver,
    doc: impl FnOnce(&std::path::Path) -> (Vec<GateDoc>, Vec<OutcomeDoc>),
) -> Fx {
    fx_doc(driver, |dir| {
        let (gates, outcomes) = doc(dir);
        GatesDoc { gates, outcomes }
    })
}

/// [`fx`] over `driver`, with the gates and outcomes `doc` gives.
fn fx_doc(driver: FakeDriver, doc: impl FnOnce(&std::path::Path) -> GatesDoc) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let probe = BoxPath::under("/work/box-1", ".moor/boxwork").unwrap();
    let driver = Arc::new(driver.peek_text("box-1", &probe, "ahead=0 dirty=0"));
    let engine = Arc::new(FakeEngine::new().probes("agent-1", agent::idle(Ts::from_millis(0))));
    let proc = Arc::new(ScriptedProc::new());
    proc.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok("/home/me/proj/.git\n".into()),
    );
    let cx = Cx::fake(dir.path(), driver.clone(), proc).with_engine(engine.clone());
    let json = serde_json::to_string(&doc(dir.path())).unwrap();
    let id = ready_box(&cx, &driver, json);
    cx.store().unwrap().begin_launch(id, "agent-1").unwrap();
    Fx {
        dir,
        cx,
        driver,
        engine,
        id,
    }
}

fn ready_box(cx: &Cx, driver: &FakeDriver, gates: String) -> BoxId {
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let new = Decision::insert(NewBox {
        name: name.clone(),
        repo: None,
        spec: SpecFields {
            gates: Some(gates),
            ..SpecFields::default()
        },
    })
    .requester(REQUESTER_USER);
    let id = store.apply(new).unwrap().box_id;
    let provider = ProviderId::new("p");
    let fields = HandsFields {
        provider: Field::Set(provider.clone()),
        workdir: Field::Set("/work/box-1".into()),
        ..HandsFields::default()
    };
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .hands_input(HandsInc::FIRST, HandsInput::Requested)
        .hands(HandsInc::FIRST, fields);
    store.apply(d).unwrap();
    let got = driver
        .acquire(&AcquireReq {
            pool_machine: None,
            provider,
            box_id: id,
            name,
            class: None,
            nonce: "n".to_owned(),
            cache: moor_core::cache::CachePlan::none(),
        })
        .unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .hands_input(HandsInc::FIRST, HandsInput::Acquired { handle: got.handle });
    store.apply(d).unwrap();
    id
}

/// What a Box busy with another command looks like to the exec op: no result, and no cut link.
pub(super) fn busy() -> DriverError {
    DriverError::Unavailable("Box 1 is busy with another command; waited 30s".to_owned())
}

/// What a lost stream looks like to the exec op.
pub(super) fn cut() -> DriverError {
    DriverError::Transport {
        stderr_tail: "the stream ended before the command did".to_owned(),
    }
}

impl Fx {
    /// The agent calls `report_step` for `gate`, with the evidence files named `names`.
    pub(super) fn report(&self, gate: &str, result: StepResult, names: &[&str]) -> StepReportValue {
        self.report_from(self.id, gate, result, names)
    }

    /// [`Self::report`] from Box `id`.
    pub(super) fn report_from(
        &self,
        id: BoxId,
        gate: &str,
        result: StepResult,
        names: &[&str],
    ) -> StepReportValue {
        let report = StepReport {
            step: StepId::parse(gate).unwrap(),
            result,
            evidence: names.iter().map(|n| (*n).to_owned()).collect(),
            note: if result == StepResult::Fail {
                "stuck".to_owned()
            } else {
                String::new()
            },
        };
        step_report(&self.cx, &StepReportReq { box_id: id, report }).unwrap()
    }

    /// What the next look in the Box's artifacts folder finds: each file with its size.
    pub(super) fn leave(&self, files: &[(&str, u64)]) {
        let sha = "a".repeat(64);
        let text = files.iter().fold(String::new(), |text, (name, size)| {
            format!("{text}{name}\t{size}\t{sha}\n")
        });
        self.driver.push_artifact_index(text);
    }

    fn fixed(&self) -> PathBuf {
        self.dir.path().join("fixed")
    }

    /// The agent fixes what the checks look for.
    pub(super) fn fix(&self) {
        std::fs::write(self.fixed(), "").unwrap();
    }

    /// Time passes.
    pub(super) fn pause(&self, secs: u64) {
        self.cx.clock().sleep(Duration::from_secs(secs));
    }

    /// The agent goes quiet a moment after it was last told anything; a read of the idle agent may
    /// have settled the Box already, which is no error.
    pub(super) fn quiet(&self) {
        self.pause(1);
        let quiet = Decision::new(self.id).session(SessionInput::AgentQuiet);
        let settled = self.cx.store().unwrap().apply(quiet).map(drop);
        assert!(
            matches!(settled, Ok(()) | Err(StoreError::Illegal(_))),
            "{settled:?}"
        );
    }

    /// One pass that runs each due check on the spot; returns what it logged.
    pub(super) fn pass(&self) -> Vec<String> {
        let ran = RefCell::new(Vec::new());
        let mut out = pass(&self.cx, self.cx.store().unwrap(), &|| false, &|job| {
            ran.borrow_mut().extend(run(&self.cx, &job));
        })
        .unwrap();
        out.extend(ran.into_inner());
        out
    }

    /// One pass that hands back the checks it would run, running none.
    pub(super) fn due(&self) -> Vec<Job> {
        let jobs = RefCell::new(Vec::new());
        let out = pass(&self.cx, self.cx.store().unwrap(), &|| false, &|job| {
            jobs.borrow_mut().push(job);
        });
        assert!(out.unwrap().is_empty());
        jobs.into_inner()
    }

    /// The gate lines written so far.
    pub(super) fn lines(&self) -> Vec<GateLine> {
        self.cx.store().unwrap().gate_lines(self.id).unwrap()
    }

    /// The exit codes of the checks written so far, each with its gate.
    pub(super) fn exits(&self) -> Vec<(String, Option<i32>)> {
        self.lines()
            .iter()
            .filter_map(|l| match l {
                GateLine::Checked { gate, exit, .. } => Some((gate.to_string(), Some(*exit))),
                GateLine::NoAnswer {
                    gate,
                    check: CheckOf::Gate { .. },
                    ..
                } => Some((gate.to_string(), None)),
                _ => None,
            })
            .collect()
    }

    /// The messages sent to the agent.
    pub(super) fn told(&self) -> Vec<String> {
        let texts = self.engine.steers().into_iter();
        texts
            .filter_map(|(_, s)| match s {
                Steer::Text(t) => Some(t),
                _ => None,
            })
            .collect()
    }

    /// A person steers the agent.
    pub(super) fn steer(&self, text: &str) {
        let req = SteerReq {
            target: moor_core::request::BoxTarget::Id(self.id),
            text: Some(text.to_owned()),
            menu: None,
            interrupt: false,
            ask: None,
        };
        crate::ops::steer::run(&self.cx, &req).unwrap();
    }

    /// Where the Box stands against its gates, and what `show` derives of it.
    pub(super) fn standing(&self) -> (GateStatus, Progress) {
        let shown = self.shown();
        (shown.gate.unwrap().status, shown.progress.unwrap().state)
    }

    /// The Box as `show` lists it.
    pub(super) fn shown(&self) -> moor_core::view::BoxView {
        let req = ShowReq {
            target: None,
            events: None,
            agent: false,
            reports: false,
            artifacts: false,
            pane: false,
            transcript: false,
        };
        let listed = crate::ops::show::run(&self.cx, &req)
            .unwrap()
            .boxes
            .unwrap();
        listed.into_iter().find(|b| b.id == self.id.get()).unwrap()
    }
}
