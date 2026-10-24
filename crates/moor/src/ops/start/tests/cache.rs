//! The cache plan a start hands the Driver, and the seal a later close runs from it.

use moor_core::cache::CachePlan;
use moor_core::config::parse_user;
use moor_core::{BoxName, CloseReq, Warning};

use super::*;

const YAML: &str = "cache: {volumes: [{name: sdk, path: /opt/sdk, seal: [rm -rf /opt/sdk/tmp]}]}\n";

fn names(plan: &CachePlan) -> Vec<&str> {
    plan.volumes().iter().map(|v| v.name.as_str()).collect()
}

fn started(w: &World) -> CachePlan {
    script_base_yaml(w, Some(YAML));
    run(&w.cx, &repo_req("demo")).unwrap();
    let plans = w.driver.acquired_plans();
    assert_eq!(plans.len(), 1);
    plans.into_iter().next().unwrap()
}

fn cache_warnings(w: &World) -> Vec<String> {
    let all = w.cx.take_warnings();
    all.into_iter()
        .filter(|w| w.code.starts_with("cache_"))
        .map(|w| w.code)
        .collect()
}

#[test]
fn a_provider_with_cache_volumes_gets_the_presets_and_the_repo_volumes() {
    let w = world(FakeDriver::new().cache_volumes());
    let plan = started(&w);
    let got = names(&plan);
    assert_eq!(got.first(), Some(&"npm"));
    assert_eq!(got.last(), Some(&"sdk"));
    assert_eq!(got.len(), 12);
    assert!(got.contains(&"worktree"), "{got:?}");
    assert!(cache_warnings(&w).is_empty());
}

#[test]
fn a_provider_without_cache_volumes_starts_cold_with_one_warning() {
    let w = world(FakeDriver::new());
    assert!(started(&w).is_empty());
    let got: Vec<Warning> = w.cx.take_warnings();
    let unavailable: Vec<&str> = got
        .iter()
        .filter(|w| w.code == "cache_unavailable")
        .map(|w| w.message.as_str())
        .collect();
    assert_eq!(unavailable.len(), 1, "{got:?}");
    assert!(
        unavailable.iter().all(|m| m.contains(" 1 declared")),
        "the presets are not counted: {unavailable:?}"
    );
}

#[test]
fn presets_alone_start_cold_without_a_warning() {
    for yaml in [None, Some("cache: {presets: [go]}\n")] {
        let w = world(FakeDriver::new());
        script_base_yaml(&w, yaml);
        run(&w.cx, &repo_req("demo")).unwrap();
        assert!(cache_warnings(&w).is_empty(), "{yaml:?}");
    }
}

#[test]
fn an_operator_volume_on_a_provider_without_cache_volumes_warns() {
    let config = parse_user(
        "u.yaml",
        "cache: {volumes: [{name: tools, path: ~/tools}]}\n",
    )
    .unwrap();
    let w = world_with(FakeDriver::new(), config);
    script_base_yaml(&w, None);
    run(&w.cx, &repo_req("demo")).unwrap();
    assert_eq!(cache_warnings(&w), ["cache_unavailable"]);
}

#[test]
fn an_operator_who_turns_cache_volumes_off_gets_a_cold_box_and_no_warning() {
    let config = parse_user(
        "u.yaml",
        "drivers: {local-container: {cache_volumes: false}}\n",
    )
    .unwrap();
    let w = world_with(FakeDriver::new().cache_volumes(), config);
    assert!(started(&w).is_empty());
    assert!(cache_warnings(&w).is_empty());
}

#[test]
fn clashing_layers_start_cold_with_a_refusal() {
    let config = parse_user(
        "u.yaml",
        "cache: {volumes: [{name: tools, path: /opt/sdk/tools}]}\n",
    )
    .unwrap();
    let w = world_with(FakeDriver::new().cache_volumes(), config);
    assert!(started(&w).is_empty());
    assert_eq!(cache_warnings(&w), ["cache_refused"]);
}

fn refusals(w: &World) -> Vec<String> {
    let all = w.cx.take_warnings();
    all.into_iter()
        .filter(|w| w.code.starts_with("cache_"))
        .map(|w| format!("{}: {}", w.code, w.message))
        .collect()
}

#[test]
fn a_volume_over_docker_data_is_refused_and_the_box_keeps_the_rest() {
    let w = world(FakeDriver::new().cache_volumes());
    let yaml = "cache: {volumes: [{name: mine, path: ~/docker}, {name: sdk, path: /opt/sdk}]}\n";
    script_base_yaml(&w, Some(yaml));
    run(&w.cx, &repo_req("demo")).unwrap();
    let plan = w.driver.acquired_plans().into_iter().next().unwrap();
    let got = names(&plan);
    assert!(got.contains(&"docker") && got.contains(&"sdk"), "{got:?}");
    assert!(!got.contains(&"mine"), "{got:?}");
    let [warned]: [String; 1] = refusals(&w).try_into().unwrap();
    assert!(
        warned.starts_with("cache_refused: repo cache volume mine meets the sealed docker"),
        "{warned}"
    );
}

#[test]
fn a_volume_over_docker_data_is_refused_with_presets_off() {
    let w = world(FakeDriver::new().cache_volumes());
    let yaml = "cache: {presets: false, volumes: [{name: mine, path: ~/docker}]}\n";
    script_base_yaml(&w, Some(yaml));
    run(&w.cx, &repo_req("demo")).unwrap();
    let [plan]: [CachePlan; 1] = w.driver.acquired_plans().try_into().unwrap();
    assert_eq!(names(&plan), ["worktree"], "only the work tree is kept");
    let [warned]: [String; 1] = refusals(&w).try_into().unwrap();
    assert!(warned.starts_with("cache_refused: "), "{warned}");
}

/// The scripts of every exec that ran a seal, in order.
fn seals(w: &World) -> Vec<String> {
    w.driver
        .execs()
        .into_iter()
        .filter_map(|(_, spec, _)| spec.argv.last().cloned())
        .filter(|s| s.contains("moor_cache_clear"))
        .collect()
}

fn close(cx: &Cx) {
    let req = CloseReq {
        target: BoxName::parse_allow_bare("demo").unwrap().into(),
        force: true,
    };
    crate::ops::close::run(cx, &req).unwrap();
}

#[test]
fn a_close_after_a_controller_restart_seals_the_plan_the_start_used() {
    let w = world(FakeDriver::new().cache_volumes());
    let plan = started(&w);
    let restarted = Cx::fake(
        w.dir.path(),
        w.driver.clone(),
        w.surface.clone(),
        w.proc.clone(),
    );
    close(&restarted);
    assert_eq!(seals(&w), [plan.seal_script(Some(BASE)).unwrap()]);
    let calls = w.driver.calls();
    let at = |p: &str| calls.iter().rposition(|c| c.starts_with(p)).unwrap();
    assert!(
        at("exec") < at("release"),
        "the seal runs before the release"
    );
}

#[test]
fn a_failed_seal_warns_and_the_close_still_releases() {
    let w = world(FakeDriver::new().cache_volumes());
    started(&w);
    w.cx.take_warnings();
    w.driver.set_run_stdout("moor: cache seal failed: docker\n");
    close(&w.cx);
    let got = w.cx.take_warnings();
    let failed: Vec<&Warning> = got
        .iter()
        .filter(|w| w.code.starts_with("cache_"))
        .collect();
    assert_eq!(failed.len(), 1, "{got:?}");
    let message = &failed.first().unwrap().message;
    assert_eq!(failed.first().unwrap().code, "cache_seal_failed");
    for says in ["docker", "not kept", "partial contents may remain"] {
        assert!(message.contains(says), "{message}");
    }
    assert!(w.driver.calls().iter().any(|c| c.starts_with("release")));
}

#[test]
fn a_cold_box_runs_no_seal() {
    let w = world(FakeDriver::new());
    started(&w);
    close(&w.cx);
    assert!(seals(&w).is_empty());
}
