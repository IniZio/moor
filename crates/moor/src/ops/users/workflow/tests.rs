#![allow(
    clippy::indexing_slicing,
    clippy::needless_pass_by_value,
    reason = "test assertions index JSON values and build a command per call"
)]

use std::time::Duration;

use moor_core::name::ForgeRepo;
use moor_core::request::{UserCmd, WorkflowAddReq, WorkflowCmd};
use moor_core::seam::clock::Ts;
use moor_core::user::Identity;
use moor_core::workflow::{
    NewRun, Plan, RunCause, RunOutcome, RunState, WorkflowId, WorkflowName, WorkflowState,
};
use moor_core::BoxId;
use serde_json::{json, Value};

use crate::ops::users::run;
use crate::ops::users::tests::{fixture, id, Fixture};
use crate::testing::runs::{refused, running, skipped, timed_out};

fn add_req(name: &str, cron: &str, tz: &str) -> WorkflowAddReq {
    WorkflowAddReq {
        identity: None,
        name: name.to_owned(),
        cron: Some(cron.to_owned()),
        timezone: Some(tz.to_owned()),
        repos: vec!["o/n".to_owned()],
        config: None,
        prompt: "fix what is broken".to_owned(),
        timeout_minutes: None,
        plan: None,
        expected_revision: None,
        origin_key: None,
        keep_state: false,
        enabled: Some(true),
        on_pull_request: Some(false),
        gates: None,
    }
}

fn cmd(c: WorkflowCmd) -> UserCmd {
    UserCmd::Workflow(c)
}

fn add(req: WorkflowAddReq, front: Option<&str>) -> UserCmd {
    cmd(WorkflowCmd::Add {
        req,
        origin_front: front.map(str::to_owned),
    })
}

fn me(f: &Fixture, who: &str) -> Identity {
    let me = id(who);
    f.cx.store().unwrap().enroll(&me, None).unwrap();
    me
}

fn ok(f: &Fixture, who: &Identity, c: UserCmd) -> Value {
    run(&f.cx, &f.vault, who, &c).unwrap()
}

fn refusal(f: &Fixture, who: &Identity, c: UserCmd) -> String {
    run(&f.cx, &f.vault, who, &c).unwrap_err().error.to_string()
}

fn daily(f: &Fixture, who: &Identity) {
    ok(
        f,
        who,
        add(add_req("daily", "0 9 * * *", "Europe/London"), None),
    );
}

#[test]
fn an_added_workflow_is_listed_with_its_next_due_time() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let added = ok(&f, &who, add(add_req("daily", "0 9 * * *", "UTC"), None));
    assert_eq!(added["name"], "daily");
    assert_eq!(added["next_due"], 9 * 3_600_000);
    let listed = ok(&f, &who, cmd(WorkflowCmd::List));
    let w = &listed["workflows"][0];
    assert_eq!(
        (
            w["name"].clone(),
            w["enabled"].clone(),
            w["timeout_minutes"].clone()
        ),
        (json!("daily"), json!(true), json!(180))
    );
    assert_eq!(w["next_due"], 9 * 3_600_000);
    assert_eq!(w.get("also"), None);
    assert_eq!(w.get("last_run"), None);
}

#[test]
fn the_repo_set_is_stored_and_listed_in_order() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let mut req = add_req("daily", "0 9 * * *", "UTC");
    req.repos = vec!["o/n".to_owned(), "o/lib".to_owned(), "p/deploy".to_owned()];
    ok(&f, &who, add(req.clone(), None));
    let listed = ok(&f, &who, cmd(WorkflowCmd::List));
    assert_eq!(
        listed["workflows"][0]["repos"],
        json!(["o/n", "o/lib", "p/deploy"])
    );
    assert_eq!(listed["workflows"][0].get("also"), None);

    req.repos = vec!["o/lib".to_owned(), "o/n".to_owned()];
    ok(&f, &who, add(req, None));
    let listed = ok(&f, &who, cmd(WorkflowCmd::List));
    assert_eq!(
        listed["workflows"][0]["repos"],
        json!(["o/lib", "o/n"]),
        "adding the name again replaces the set and its order"
    );
}

#[test]
fn a_repo_of_the_set_that_would_share_a_folder_is_refused() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let try_add = |repos: &[&str]| {
        let mut req = add_req("daily", "0 9 * * *", "UTC");
        req.repos = repos.iter().map(|s| (*s).to_owned()).collect();
        refusal(&f, &who, add(req, None))
    };
    assert!(try_add(&["o/n", "o/lib", "nope"]).contains("repos[2]"));
    assert!(try_add(&["o/n", "o/n"]).contains("o/n"));
    let clash = try_add(&["o/n", "p/N"]);
    assert!(clash.contains("o/n") && clash.contains("p/N"), "{clash}");
    let listed = ok(&f, &who, cmd(WorkflowCmd::List));
    assert_eq!(listed["workflows"], json!([]), "nothing was stored");
}

#[test]
fn a_refused_add_names_the_bad_field() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let try_add = |req: WorkflowAddReq| refusal(&f, &who, add(req, None));
    let bad_cron = try_add(add_req("a", "61 * * * *", "UTC"));
    assert!(bad_cron.contains("61 * * * *"), "{bad_cron}");
    let bad_zone = try_add(add_req("a", "0 9 * * *", "Mars/Olympus"));
    assert!(bad_zone.contains("Mars/Olympus"), "{bad_zone}");
    let fast = try_add(add_req("a", "* * * * *", "UTC"));
    assert!(
        fast.contains("* * * * *") && fast.contains("60 minutes"),
        "{fast}"
    );
    let mut repos = add_req("a", "0 9 * * *", "UTC");
    repos.repos = vec!["o/n".to_owned(), "nope".to_owned()];
    assert!(try_add(repos).contains("repos[1]"));
    let mut slow = add_req("a", "0 9 * * *", "UTC");
    slow.timeout_minutes = Some(721);
    assert!(try_add(slow).contains("721"));
    assert!(try_add(add_req("Bad Name", "0 9 * * *", "UTC")).contains("Bad Name"));
    let listed = ok(&f, &who, cmd(WorkflowCmd::List));
    assert_eq!(listed["workflows"], json!([]), "nothing was stored");
}

#[test]
fn a_lower_floor_lets_a_one_minute_cron_through() {
    let mut f = fixture();
    f.cx =
        f.cx.with_env(|e| e.with_workflow_min_interval(Duration::from_secs(60)));
    let who = me(&f, "local:2000");
    ok(&f, &who, add(add_req("fast", "* * * * *", "UTC"), None));
    let again = refusal(
        &f,
        &who,
        add(add_req("fast2", "*/30 * * * * *", "UTC"), None),
    );
    assert!(again.contains("*/30 * * * * *"), "{again}");
}

#[test]
fn a_person_sees_and_changes_only_their_own_workflows() {
    let f = fixture();
    let (alice, bob) = (me(&f, "local:2000"), me(&f, "local:2001"));
    daily(&f, &alice);
    assert_eq!(ok(&f, &bob, cmd(WorkflowCmd::List))["workflows"], json!([]));
    let named = |name: &str| WorkflowCmd::SetEnabled {
        name: name.to_owned(),
        enabled: false,
    };
    assert!(refusal(&f, &bob, cmd(named("daily"))).contains("no workflow named daily"));
    let removed = ok(
        &f,
        &bob,
        cmd(WorkflowCmd::Remove {
            name: "daily".to_owned(),
        }),
    );
    assert_eq!(removed["changed"], false);
    ok(&f, &alice, cmd(named("daily")));
    let w = &ok(&f, &alice, cmd(WorkflowCmd::List))["workflows"][0];
    assert_eq!(w["enabled"], false);
    assert!(w.get("next_due").is_none());
    let gone = ok(
        &f,
        &alice,
        cmd(WorkflowCmd::Remove {
            name: "daily".to_owned(),
        }),
    );
    assert_eq!(gone["changed"], true);
}

#[test]
fn an_origin_is_kept_only_from_a_front_that_names_a_key() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let mut req = add_req("daily", "0 9 * * *", "UTC");
    ok(&f, &who, add(req.clone(), Some("herdr")));
    let store = f.cx.store().unwrap();
    let origin = |s: &crate::store::Store| {
        s.workflow(
            s.user_of(&id("local:2000")).unwrap().unwrap(),
            &WorkflowName::parse("daily").unwrap(),
        )
        .unwrap()
        .unwrap()
        .origin
    };
    assert_eq!(origin(store), None, "a front without a key records nothing");

    req.origin_key = Some("C1/T1".to_owned());
    assert!(refusal(&f, &who, add(req.clone(), None)).contains("only a front"));
    ok(&f, &who, add(req, Some("herdr")));
    let got = origin(store).unwrap();
    assert_eq!(
        (got.front.as_str(), got.key.as_deref()),
        ("herdr", Some("C1/T1"))
    );
}

/// The stored workflow `daily` of `who`.
fn stored(f: &Fixture, who: &Identity) -> moor_core::workflow::Workflow {
    let store = f.cx.store().unwrap();
    store
        .workflow(
            store.user_of(who).unwrap().unwrap(),
            &WorkflowName::parse("daily").unwrap(),
        )
        .unwrap()
        .unwrap()
}

#[test]
fn a_replace_that_sends_no_origin_keeps_the_stored_one() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let mut req = add_req("daily", "0 9 * * *", "UTC");
    req.origin_key = Some("C1/T1".to_owned());
    ok(&f, &who, add(req.clone(), Some("slack")));

    req.origin_key = None;
    req.prompt = "a new prompt".to_owned();
    ok(&f, &who, add(req.clone(), None));
    let got = stored(&f, &who);
    assert_eq!(got.prompt, "a new prompt");
    let origin = got.origin.unwrap();
    assert_eq!(
        (origin.front.as_str(), origin.key.as_deref()),
        ("slack", Some("C1/T1")),
        "a caller that is no front, and sends no key, leaves the origin"
    );

    req.origin_key = Some("C2/T2".to_owned());
    ok(&f, &who, add(req, Some("slack")));
    let origin = stored(&f, &who).origin.unwrap();
    assert_eq!(
        origin.key.as_deref(),
        Some("C2/T2"),
        "a front's key still sets it"
    );
}

#[test]
fn keep_state_leaves_a_disabled_workflow_disabled() {
    let f = fixture();
    let who = me(&f, "local:2000");
    daily(&f, &who);
    let store = f.cx.store().unwrap();
    assert!(store
        .set_workflow_enabled(
            store.user_of(&who).unwrap().unwrap(),
            &WorkflowName::parse("daily").unwrap(),
            false,
        )
        .unwrap());
    let disabled = stored(&f, &who).state;

    let mut req = add_req("daily", "0 9 * * *", "UTC");
    req.prompt = "edited".to_owned();
    req.keep_state = true;
    ok(&f, &who, add(req.clone(), None));
    let got = stored(&f, &who);
    assert_eq!(got.prompt, "edited");
    assert_eq!(got.state, disabled, "a kept state stays disabled");

    req.keep_state = false;
    ok(&f, &who, add(req, None));
    assert!(
        matches!(stored(&f, &who).state, WorkflowState::Enabled { .. }),
        "add_workflow as the MCP tool sends it still enables the workflow"
    );
}

fn record(f: &Fixture, who: &Identity, state: RunState, due: i64) -> WorkflowId {
    let store = f.cx.store().unwrap();
    let user = store.user_of(who).unwrap().unwrap();
    let w = store
        .workflow(user, &WorkflowName::parse("daily").unwrap())
        .unwrap()
        .unwrap();
    let run = NewRun {
        workflow: w.id,
        repo: ForgeRepo::new("o", "n"),
        due_at: Ts::from_millis(due),
        state,
        recorded_at: Ts::from_millis(due),
        plan: None,
        cause: RunCause::Cron,
    };
    store.record_workflow_run(&run).unwrap();
    w.id
}

fn runs(f: &Fixture, who: &Identity, after: Option<i64>, limit: Option<u32>) -> Value {
    ok(
        f,
        who,
        cmd(WorkflowCmd::Runs {
            after,
            limit,
            workflow: None,
        }),
    )
}

#[test]
fn the_stream_shows_a_run_again_each_time_it_moves_on() {
    let f = fixture();
    let who = me(&f, "local:2000");
    daily(&f, &who);
    let id = record(&f, &who, RunState::Queued, 100);
    let first = runs(&f, &who, None, None);
    assert_eq!(first["changes"][0]["outcome"], "starting");
    let cursor = first["next"].as_i64().unwrap();
    assert_eq!(runs(&f, &who, Some(cursor), None)["changes"], json!([]));

    let store = f.cx.store().unwrap();
    let key = crate::testing::runs::run_at(
        f.cx.store().unwrap(),
        (id, &ForgeRepo::new("o", "n"), Ts::from_millis(100)),
    );
    store
        .resolve_workflow_start(key, &running(9), Ts::from_millis(150))
        .unwrap();
    let second = runs(&f, &who, Some(cursor), None);
    assert_eq!(second["changes"].as_array().unwrap().len(), 1);
    assert_eq!(second["changes"][0]["outcome"], "started");
    assert_eq!(second["changes"][0]["workflow"], "daily");
    assert!(second["next"].as_i64().unwrap() > cursor);

    record(&f, &who, skipped(), 200);
    record(&f, &who, refused("full"), 300);
    let all = runs(&f, &who, None, None);
    let outcomes: Vec<_> = all["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["outcome"].clone())
        .collect();
    assert_eq!(
        outcomes,
        [json!("started"), json!("skipped_overlap"), json!("refused")]
    );
    assert_eq!(all["changes"][2]["detail"], "full");
    let page = runs(&f, &who, None, Some(1));
    assert_eq!(page["changes"].as_array().unwrap().len(), 1);
}

#[test]
fn a_run_shows_its_phase_its_step_its_result_and_its_history() {
    let f = fixture();
    let who = me(&f, "local:2000");
    daily(&f, &who);
    let id = record(&f, &who, RunState::Queued, 100);
    let store = f.cx.store().unwrap();
    let key = crate::testing::runs::run_at(
        f.cx.store().unwrap(),
        (id, &ForgeRepo::new("o", "n"), Ts::from_millis(100)),
    );
    store
        .resolve_workflow_start(key, &running(9), Ts::from_millis(150))
        .unwrap();
    let last_run = || ok(&f, &who, cmd(WorkflowCmd::List))["workflows"][0]["last_run"].clone();
    let queued = json!({"at": 100, "entry": {"kind": "queued"}});
    let entered = json!({"at": 150, "entry": {"kind": "entered", "step": "deliver", "try_n": 1}});
    assert_eq!(
        last_run(),
        json!({
            "due_at": 100, "outcome": "started", "phase": "running",
            "trigger": "cron", "step": "deliver", "history": [queued, entered],
        })
    );

    let delivered = RunOutcome::Delivered {
        result: "opened a pull request".to_owned(),
        verdict: None,
    };
    store
        .end_workflow_run(
            key,
            (&Plan::single_id(), 1),
            (BoxId::new(9), delivered),
            (&[], Ts::from_millis(900)),
        )
        .unwrap();
    let ended = json!({"at": 900, "entry": {
        "kind": "ended", "phase_before": "running",
        "outcome": {"outcome": "delivered", "result": "opened a pull request"},
    }});
    let done = json!({
        "due_at": 100, "outcome": "finished", "phase": "done", "trigger": "cron",
        "result": "opened a pull request", "history": [queued, entered, ended],
    });
    assert_eq!(last_run(), done);
    let streamed = runs(&f, &who, None, None);
    let change = streamed["changes"].as_array().unwrap().last().unwrap();
    assert_eq!(change["history"], done["history"]);
}

fn run_list(f: &Fixture, who: &Identity, only: Option<i64>) -> Value {
    let list = WorkflowCmd::RunList {
        workflow: "daily".to_owned(),
        run: only,
        limit: None,
    };
    ok(f, who, cmd(list))
}

#[test]
fn the_run_list_is_newest_first_and_one_run_reads_with_its_frozen_plan() {
    let f = fixture();
    let who = me(&f, "local:2000");
    daily(&f, &who);
    record(&f, &who, RunState::Queued, 100);
    record(&f, &who, RunState::Queued, 300);
    record(&f, &who, RunState::Queued, 200);
    let listed = run_list(&f, &who, None);
    let due: Vec<i64> = listed["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["due_at"].as_i64().unwrap())
        .collect();
    assert_eq!(due, [300, 200, 100]);
    assert!(listed["runs"][0].get("plan").is_none());
    let id = listed["runs"][1]["id"].as_i64().unwrap();
    let one = run_list(&f, &who, Some(id));
    assert_eq!(one["runs"].as_array().unwrap().len(), 1);
    assert_eq!(one["runs"][0]["due_at"], 200);
    assert_eq!(one["runs"][0]["plan"]["steps"][0]["kind"], "deliver");
}

#[test]
fn the_run_list_of_a_workflow_that_is_not_yours_is_refused() {
    let f = fixture();
    let owner = me(&f, "local:2000");
    daily(&f, &owner);
    let other = me(&f, "local:2001");
    let why = refusal(
        &f,
        &other,
        cmd(WorkflowCmd::RunList {
            workflow: "daily".to_owned(),
            run: None,
            limit: None,
        }),
    );
    assert!(why.contains("no workflow named daily"), "{why}");
}

#[test]
fn five_failed_runs_leave_the_workflow_enabled_and_each_shows_in_the_stream() {
    let f = fixture();
    let who = me(&f, "local:2000");
    daily(&f, &who);
    for due in 1..=3 {
        record(&f, &who, refused("no repo"), due * 100);
    }
    for due in 4..=5 {
        record(&f, &who, timed_out(9, due * 100), due * 100);
    }
    let all = runs(&f, &who, None, None);
    let outcomes: Vec<_> = all["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(
        outcomes,
        ["refused", "refused", "refused", "timed_out", "timed_out"]
    );
    let listed = ok(&f, &who, cmd(WorkflowCmd::List));
    assert_eq!(listed["workflows"][0]["enabled"], true);
    assert!(listed["workflows"][0].get("disabled_reason").is_none());
}

#[test]
fn a_starting_or_refused_run_names_its_planned_box_and_no_other_does() {
    let f = fixture();
    let who = me(&f, "local:2000");
    daily(&f, &who);
    record(&f, &who, RunState::Queued, 100);
    record(&f, &who, skipped(), 200);
    record(&f, &who, refused("full"), 300);
    let id = record(&f, &who, running(9), 400);
    let all = runs(&f, &who, None, None);
    let seen: Vec<_> = all["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["outcome"].clone(),
                c["planned_box"].clone(),
                c["box"].clone(),
            )
        })
        .collect();
    let planned = |due| {
        let name =
            moor_core::workflow::box_name(id, "daily", Ts::from_millis(due), &RunCause::Cron);
        json!(name.unwrap().as_str())
    };
    assert_eq!(
        seen,
        [
            (json!("starting"), planned(100), Value::Null),
            (json!("skipped_overlap"), Value::Null, Value::Null),
            (json!("refused"), planned(300), Value::Null),
            (json!("started"), Value::Null, Value::Null),
        ]
    );
    let listed = ok(&f, &who, cmd(WorkflowCmd::List));
    assert!(listed["workflows"][0]["last_runs"][0].get("box").is_none());
}

mod enabled_and_filter;
mod gates;
mod keep_omitted;
mod no_cron;
mod plan;
