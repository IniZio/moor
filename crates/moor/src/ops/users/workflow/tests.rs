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
use moor_core::workflow::{WorkflowId, WorkflowName, WorkflowOutcome, WorkflowRun};
use moor_core::BoxId;
use serde_json::{json, Value};

use crate::ops::users::run;
use crate::ops::users::tests::{fixture, id, Fixture};

fn add_req(name: &str, cron: &str, tz: &str) -> WorkflowAddReq {
    WorkflowAddReq {
        identity: None,
        name: name.to_owned(),
        cron: cron.to_owned(),
        timezone: tz.to_owned(),
        repos: vec!["o/n".to_owned()],
        prompt: "fix what is broken".to_owned(),
        timeout_minutes: None,
        origin_key: None,
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
    assert_eq!(w["last_runs"], json!([]));
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
    f.cx = f.cx.with_workflow_floor(Duration::from_secs(60));
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
    ok(&f, &who, add(req.clone(), Some("slack")));
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
    ok(&f, &who, add(req, Some("slack")));
    let got = origin(store).unwrap();
    assert_eq!(
        (got.front.as_str(), got.key.as_deref()),
        ("slack", Some("C1/T1"))
    );
}

fn record(f: &Fixture, who: &Identity, outcome: WorkflowOutcome, due: i64) -> WorkflowId {
    let store = f.cx.store().unwrap();
    let user = store.user_of(who).unwrap().unwrap();
    let w = store
        .workflow(user, &WorkflowName::parse("daily").unwrap())
        .unwrap()
        .unwrap();
    let run = WorkflowRun {
        workflow: w.id,
        repo: ForgeRepo::new("o", "n"),
        due_at: Ts::from_millis(due),
        outcome,
        recorded_at: Ts::from_millis(due),
    };
    store.record_workflow_run(&run).unwrap();
    w.id
}

fn runs(f: &Fixture, who: &Identity, after: Option<i64>, limit: Option<u32>) -> Value {
    ok(f, who, cmd(WorkflowCmd::Runs { after, limit }))
}

#[test]
fn the_stream_shows_a_run_again_each_time_it_moves_on() {
    let f = fixture();
    let who = me(&f, "local:2000");
    daily(&f, &who);
    let id = record(&f, &who, WorkflowOutcome::Starting, 100);
    let first = runs(&f, &who, None, None);
    assert_eq!(first["changes"][0]["outcome"], "starting");
    let cursor = first["next"].as_i64().unwrap();
    assert_eq!(runs(&f, &who, Some(cursor), None)["changes"], json!([]));

    let store = f.cx.store().unwrap();
    let key = (id, &ForgeRepo::new("o", "n"), Ts::from_millis(100));
    let started = WorkflowOutcome::Started {
        box_id: BoxId::new(9),
    };
    store
        .resolve_workflow_start(key, &started, Ts::from_millis(150))
        .unwrap();
    let second = runs(&f, &who, Some(cursor), None);
    assert_eq!(second["changes"].as_array().unwrap().len(), 1);
    assert_eq!(second["changes"][0]["outcome"], "started");
    assert_eq!(second["changes"][0]["workflow"], "daily");
    assert!(second["next"].as_i64().unwrap() > cursor);

    record(&f, &who, WorkflowOutcome::SkippedOverlap, 200);
    record(
        &f,
        &who,
        WorkflowOutcome::Refused {
            detail: "full".to_owned(),
        },
        300,
    );
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
fn the_breaker_disabling_is_a_change_of_its_own_and_other_people_see_none() {
    let f = fixture();
    let (alice, bob) = (me(&f, "local:2000"), me(&f, "local:2001"));
    daily(&f, &alice);
    let id = record(&f, &alice, WorkflowOutcome::SkippedOverlap, 100);
    let cursor = runs(&f, &alice, None, None)["next"].as_i64().unwrap();
    f.cx.store()
        .unwrap()
        .disable_workflow(id, "o/n: three failures")
        .unwrap();
    let after = runs(&f, &alice, Some(cursor), None);
    assert_eq!(after["changes"][0]["kind"], "disabled");
    assert_eq!(after["changes"][0]["reason"], "o/n: three failures");
    assert_eq!(after["changes"][0]["workflow"], "daily");
    assert_eq!(runs(&f, &bob, None, None)["changes"], json!([]));
    let listed = ok(&f, &alice, cmd(WorkflowCmd::List));
    assert_eq!(
        listed["workflows"][0]["disabled_reason"],
        "o/n: three failures"
    );
}

#[test]
fn a_starting_or_refused_run_names_its_planned_box_and_no_other_does() {
    let f = fixture();
    let who = me(&f, "local:2000");
    daily(&f, &who);
    record(&f, &who, WorkflowOutcome::Starting, 100);
    record(&f, &who, WorkflowOutcome::SkippedOverlap, 200);
    record(
        &f,
        &who,
        WorkflowOutcome::Refused {
            detail: "full".to_owned(),
        },
        300,
    );
    let id = record(
        &f,
        &who,
        WorkflowOutcome::Started {
            box_id: BoxId::new(9),
        },
        400,
    );
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
        let name = moor_core::workflow::box_name(
            id,
            "daily",
            &ForgeRepo::new("o", "n"),
            Ts::from_millis(due),
        );
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
