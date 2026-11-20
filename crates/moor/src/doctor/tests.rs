//! Shared fakes and the report-level tests.

use moor_core::config::{parse_user, UserConfig};
use moor_core::{ExitClass, Request, Response};
use serde_json::{json, Value};

use super::*;
use crate::cli::GlobalOpts;

mod on_host;
mod on_images;
mod on_nodes;
mod on_setup;
mod on_user;

/// An envelope that answered `cmd` with `value`.
#[expect(
    clippy::unnecessary_wraps,
    reason = "it has the shape of a Script's return"
)]
pub(super) fn answered(cmd: &str, value: Value) -> Result<Response, ClientError> {
    Ok(Response::ok(cmd, Some(value)))
}

/// What the controller says to each request.
pub(super) type Script = Box<dyn Fn(Request) -> Result<Response, ClientError>>;

/// What an agent at a path reports as its version.
pub(super) type VersionOf = Box<dyn Fn(&str) -> Option<String>>;

/// What a GET of the gate answers.
pub(super) type Get = Box<dyn Fn(&str) -> Result<u16, String>>;

/// Everything the checks read, owned so a test can change one part.
pub(super) struct Rig {
    pub env: Env,
    pub config: UserConfig,
    pub config_error: Option<String>,
    pub target: Option<Target>,
    pub script: Script,
    pub public_url: Option<String>,
    pub repo: Option<moor_core::name::ForgeRepo>,
    pub get: Get,
    pub present: Box<dyn Fn(&str) -> images::Presence + Sync>,
    pub agent_version: VersionOf,
    pub images_fail: bool,
}

impl Rig {
    /// A host with no controller.
    pub(super) fn new() -> Self {
        Self {
            env: Env::from_pairs([("MOOR_CONFIG", "/nonexistent/moor.yaml")]),
            config: UserConfig::default(),
            config_error: None,
            target: None,
            script: Box::new(|req| Err(ClientError::Usage(format!("unscripted {}", req.cmd())))),
            public_url: None,
            repo: None,
            get: Box::new(|_| Err("unscripted".to_owned())),
            present: Box::new(|_| images::Presence::Local),
            agent_version: Box::new(|_| None),
            images_fail: true,
        }
    }

    /// The same host with a controller at `/run/moor.sock` answering through `script`.
    pub(super) fn with_controller(mut self, script: Script) -> Self {
        self.target = Some(Target::Socket("/run/moor.sock".into()));
        self.script = script;
        self
    }

    /// The same host with `yaml` as its user config.
    pub(super) fn with_config(mut self, yaml: &str) -> Self {
        self.config = parse_user("test", yaml).expect("config parses");
        self
    }

    /// Runs `f` on the inputs of this rig.
    pub(super) fn with<T>(&self, f: impl FnOnce(&Inputs<'_>) -> T) -> T {
        f(&Inputs {
            env: &self.env,
            config: &self.config,
            config_error: self.config_error.as_deref(),
            target: self.target.as_ref(),
            ask: &*self.script,
            get: &*self.get,
            public_url: self.public_url.as_deref(),
            repo: self.repo.clone(),
            present: &*self.present,
            agent_version: &*self.agent_version,
            images_fail: self.images_fail,
        })
    }

    /// The whole report.
    pub(super) fn report(&self) -> Vec<Check> {
        self.with(checks)
    }
}

#[test]
fn checks_run_in_the_order_of_the_setup_guide() {
    let rig = Rig::new().with_controller(Box::new(|r| {
        answered(
            r.cmd(),
            json!({"version": env!("CARGO_PKG_VERSION"), "nodes": []}),
        )
    }));
    let report = rig.report();
    let names: Vec<_> = report.iter().map(|c| c.name).collect();
    assert_eq!(names, ["config", "controller", "egress", "nodes", "user"]);
}

#[test]
fn gate_is_checked_only_when_its_url_is_known() {
    let mut rig = Rig::new();
    assert!(rig.report().iter().all(|c| c.name != "gate"));
    rig.public_url = Some("https://moor.example".to_owned());
    assert_eq!(rig.report().last().map(|c| c.name), Some("gate"));
}

fn report_of(statuses: &[Status]) -> Vec<Check> {
    statuses
        .iter()
        .map(|s| Check {
            name: "controller",
            status: *s,
            detail: "moor 1".to_owned(),
            fix: if *s == Status::Ok {
                String::new()
            } else {
                "do it".to_owned()
            },
        })
        .collect()
}

#[test]
fn text_report_is_aligned_with_the_fix_beside_the_detail() {
    let out = render::output(
        &report_of(&[Status::Ok, Status::Fail]),
        &GlobalOpts::default(),
    );
    assert_eq!(
        out.stdout,
        "ok    controller  moor 1\nfail  controller  moor 1; fix: do it\n"
    );
}

#[test]
fn exit_code_is_one_only_when_a_check_failed() {
    let opts = GlobalOpts::default();
    assert_eq!(
        render::output(&report_of(&[Status::Ok, Status::Warn]), &opts).code,
        0
    );
    assert_eq!(
        render::output(&report_of(&[Status::Warn, Status::Fail]), &opts).code,
        1
    );
}

#[test]
fn json_report_is_a_list_of_check_status_detail_fix() {
    let opts = GlobalOpts {
        json: true,
        ..GlobalOpts::default()
    };
    let out = render::output(&report_of(&[Status::Warn]), &opts);
    let env: Value = serde_json::from_str(&out.stdout).expect("one envelope");
    assert_eq!(env.get("cmd"), Some(&json!("doctor")));
    assert_eq!(
        env.get("value"),
        Some(
            &json!([{"check": "controller", "status": "warn", "detail": "moor 1", "fix": "do it"}])
        )
    );
    assert_eq!(out.code, 0);
    let failed = render::output(&report_of(&[Status::Fail]), &opts);
    assert_eq!(failed.code, 1);
    let class = serde_json::from_str::<Value>(&failed.stdout).expect("envelope");
    assert_eq!(class.get("class"), Some(&json!(ExitClass::Ok)));
}
