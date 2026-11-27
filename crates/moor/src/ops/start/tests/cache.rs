//! The cache plan a start hands the Driver, and the restore its provisioning runs from it.

use moor_core::cache::CachePlan;
use moor_core::config::parse_user;
use moor_core::seam::driver::ExecOutcome;
use moor_core::view::StartView;
use moor_core::{BoxId, BoxName, CloseReq, Warning};
use serde_json::json;

use super::*;

const YAML: &str =
    "cache: {volumes: [{name: sdk, path: /opt/sdk, restore: [rm -rf /opt/sdk/tmp]}]}\n";

fn names(plan: &CachePlan) -> Vec<&str> {
    plan.volumes().iter().map(|v| v.name.as_str()).collect()
}

fn started(w: &World) -> CachePlan {
    script_repo(w, Some(YAML));
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

/// The `cache_cold` payloads of the Box a start made.
fn cold_events(w: &World, v: &StartView) -> Vec<serde_json::Value> {
    super::events::events(w, BoxId::new(v.boxed.id), "cache_cold")
}

fn reasons(w: &World, v: &StartView) -> Vec<String> {
    cold_events(w, v)
        .iter()
        .map(|p| p["reason"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_provider_with_cache_volumes_gets_the_presets_and_the_repo_volumes() {
    let w = world(FakeDriver::new().cache_volumes());
    let plan = started(&w);
    let got = names(&plan);
    assert_eq!(got.first(), Some(&"cache"));
    assert_eq!(got.last(), Some(&"sdk"));
    assert_eq!(got.len(), 17);
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
        script_repo(&w, yaml);
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
    script_repo(&w, None);
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
    script_repo(&w, Some(yaml));
    run(&w.cx, &repo_req("demo")).unwrap();
    let plan = w.driver.acquired_plans().into_iter().next().unwrap();
    let got = names(&plan);
    assert!(got.contains(&"docker") && got.contains(&"sdk"), "{got:?}");
    assert!(!got.contains(&"mine"), "{got:?}");
    let [warned]: [String; 1] = refusals(&w).try_into().unwrap();
    assert!(
        warned.starts_with("cache_refused: repo cache volume mine meets the docker"),
        "{warned}"
    );
}

#[test]
fn a_volume_over_docker_data_is_refused_with_presets_off() {
    let w = world(FakeDriver::new().cache_volumes());
    let yaml = "cache: {presets: false, volumes: [{name: mine, path: ~/docker}]}\n";
    script_repo(&w, Some(yaml));
    run(&w.cx, &repo_req("demo")).unwrap();
    let [plan]: [CachePlan; 1] = w.driver.acquired_plans().try_into().unwrap();
    assert!(names(&plan).is_empty(), "{:?}", names(&plan));
    let [warned]: [String; 1] = refusals(&w).try_into().unwrap();
    assert!(warned.starts_with("cache_refused: "), "{warned}");
}

fn restored_execs(w: &World) -> usize {
    w.driver
        .execs()
        .into_iter()
        .filter_map(|(_, spec, _)| spec.argv.last().cloned())
        .filter(|s| s.contains("moor_cache_restore"))
        .count()
}

fn close(cx: &Cx) {
    let req = CloseReq {
        target: BoxName::parse_allow_bare("demo").unwrap().into(),
        force: true,
    };
    crate::ops::close::run(cx, &req).unwrap();
}

#[test]
fn a_box_restores_its_caches_before_it_clones() {
    let w = world(FakeDriver::new().cache_volumes());
    let plan = started(&w);
    let (_, script) = clone_exec(&w);
    let restore = script.find(&plan.restore_script().unwrap()).expect(&script);
    assert!(restore < script.find("git clone").unwrap(), "{script}");
    assert_eq!(restored_execs(&w), 1);
}

#[test]
fn a_close_runs_nothing_for_the_caches_and_releases() {
    let w = world(FakeDriver::new().cache_volumes());
    started(&w);
    let before = w.driver.execs().len();
    close(&w.cx);
    let scripts = w.driver.execs();
    let after = scripts.get(before..).unwrap();
    assert!(
        !after
            .iter()
            .filter_map(|(_, spec, _)| spec.argv.last())
            .any(|s| s.contains("moor_cache_")),
        "{after:?}"
    );
    assert!(w.driver.calls().iter().any(|c| c.starts_with("release")));
}

#[test]
fn a_box_with_no_repo_still_restores_its_caches() {
    let config = parse_user(
        "u.yaml",
        "cache: {volumes: [{name: tools, path: ~/tools, restore: [\"true\"]}]}\n",
    )
    .unwrap();
    let w = world_with(FakeDriver::new().cache_volumes(), config);
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(restored_execs(&w), 1);
}

#[test]
fn a_box_with_no_repo_gets_the_tools_preset_so_a_later_one_starts_warm() {
    let w = world(FakeDriver::new().cache_volumes());
    run(&w.cx, &req("demo")).unwrap();
    let plans = w.driver.acquired_plans();
    assert_eq!(plans.len(), 1);
    assert_eq!(names(plans.first().unwrap()), ["tools"]);
    assert!(cache_warnings(&w).is_empty());
}

#[test]
fn a_cold_box_runs_no_restore() {
    let w = world(FakeDriver::new());
    started(&w);
    assert_eq!(restored_execs(&w), 0);
}

#[test]
fn an_emptied_volume_warns_and_records_and_the_box_starts() {
    let printed = "moor: cache emptied docker: blob abc does not match its hash\n";
    let w = world(FakeDriver::new().cache_volumes().exec_stdout([printed]));
    script_repo(&w, Some(YAML));
    let view = run(&w.cx, &repo_req("demo")).unwrap();
    let got = w.cx.take_warnings();
    let warned: Vec<&Warning> = got.iter().filter(|w| w.code == "cache_emptied").collect();
    let [warned]: [&Warning; 1] = warned.try_into().unwrap();
    assert_eq!(
        warned.message,
        "cache volume docker was emptied: blob abc does not match its hash; the Box goes on \
         without it"
    );
    let events = super::events::events(&w, BoxId::new(view.boxed.id), "cache_emptied");
    assert_eq!(
        events,
        [json!({"volume": "docker", "reason": "blob abc does not match its hash"})]
    );
}

#[test]
fn an_emptied_volume_is_reported_even_when_the_clone_fails() {
    let printed = "moor: cache emptied docker: bad\n";
    let w = world(
        FakeDriver::new()
            .cache_volumes()
            .exec_stdout([printed])
            .exec_results([Ok(ExecOutcome::Exited { code: 1 })]),
    );
    script_repo(&w, Some(YAML));
    assert!(run(&w.cx, &repo_req("demo")).is_err());
    let got = w.cx.take_warnings();
    assert!(got.iter().any(|w| w.code == "cache_emptied"), "{got:?}");
}

/// The Box and the script of the clone exec, the one that runs `git clone`.
fn clone_exec(w: &World) -> (String, String) {
    let execs = w.driver.execs().into_iter();
    let mut found =
        execs.filter_map(|(h, spec, _)| Some((h.as_str().to_owned(), spec.argv.last()?.clone())));
    found.find(|(_, s)| s.contains("git clone")).unwrap()
}

#[test]
fn a_box_with_the_git_preset_clones_through_its_mirror() {
    let w = world(FakeDriver::new().cache_volumes());
    started(&w);
    let (_, script) = clone_exec(&w);
    assert!(
        script.contains("moor_mirror=\"$HOME\"/'.local/share/git-mirror/"),
        "{script}"
    );
    assert!(
        script.contains("--reference \"$moor_mirror\" --dissociate"),
        "{script}"
    );
}

#[test]
fn a_box_that_drops_the_git_preset_clones_from_the_remote_alone() {
    let w = world(FakeDriver::new().cache_volumes());
    script_repo(&w, Some("cache: {presets: [git]}\n"));
    run(&w.cx, &repo_req("demo")).unwrap();
    assert!(!clone_exec(&w).1.contains("moor_mirror"));
}

#[test]
fn a_clone_that_went_without_the_mirror_warns_why() {
    let printed = "moor: git mirror skipped: git fetch failed\n";
    let w = world(FakeDriver::new().cache_volumes().exec_stdout([printed]));
    script_repo(&w, Some(YAML));
    run(&w.cx, &repo_req("demo")).unwrap();
    let warned: Vec<String> =
        w.cx.take_warnings()
            .into_iter()
            .filter(|w| w.code == "git_mirror")
            .map(|w| w.message)
            .collect();
    assert_eq!(warned, ["cloned without the git mirror: git fetch failed"]);
}

#[test]
fn the_clone_log_is_still_removed_after_the_outcome_is_read() {
    let w = world(
        FakeDriver::new()
            .cache_volumes()
            .exec_stdout(["moor: work tree refreshed\n"]),
    );
    script_repo(&w, Some(YAML));
    run(&w.cx, &repo_req("demo")).unwrap();
    assert!(!w.dir.path().join("clone").join("1.log").exists());
}

#[test]
fn a_preset_only_box_on_a_provider_without_cache_volumes_records_why_it_is_cold() {
    for yaml in [None, Some("cache: {presets: [go]}\n")] {
        let w = world(FakeDriver::new());
        script_repo(&w, yaml);
        let v = run(&w.cx, &repo_req("demo")).unwrap();
        assert_eq!(reasons(&w, &v), ["provider_unsupported"], "{yaml:?}");
        let detail = cold_events(&w, &v).first().unwrap()["detail"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(detail.contains("local-container"), "{detail}");
        assert!(cache_warnings(&w).is_empty(), "{yaml:?}");
    }
}

#[test]
fn a_box_whose_operator_switched_cache_volumes_off_records_why_it_is_cold() {
    let config = parse_user(
        "u.yaml",
        "drivers: {local-container: {cache_volumes: false}}\n",
    )
    .unwrap();
    let w = world_with(FakeDriver::new().cache_volumes(), config);
    script_repo(&w, None);
    let v = run(&w.cx, &repo_req("demo")).unwrap();
    assert_eq!(reasons(&w, &v), ["switched_off"]);
    assert!(cache_warnings(&w).is_empty());
}

#[test]
fn a_clash_records_why_the_box_is_cold_next_to_its_warning() {
    let config = parse_user(
        "u.yaml",
        "cache: {volumes: [{name: tools, path: /opt/sdk/tools}]}\n",
    )
    .unwrap();
    let w = world_with(FakeDriver::new().cache_volumes(), config);
    script_repo(&w, Some(YAML));
    let v = run(&w.cx, &repo_req("demo")).unwrap();
    assert_eq!(reasons(&w, &v), ["refused"]);
    assert_eq!(cache_warnings(&w), ["cache_refused"]);
}

#[test]
fn a_box_with_no_volumes_to_keep_records_nothing() {
    let off = "cache: {presets: false, work_tree: false}\n";
    for driver in [FakeDriver::new(), FakeDriver::new().cache_volumes()] {
        let w = world(driver);
        script_repo(&w, Some(off));
        let v = run(&w.cx, &repo_req("demo")).unwrap();
        assert!(cold_events(&w, &v).is_empty());
    }
}

#[test]
fn a_box_that_keeps_its_volumes_records_nothing() {
    let w = world(FakeDriver::new().cache_volumes());
    script_repo(&w, Some(YAML));
    let v = run(&w.cx, &repo_req("demo")).unwrap();
    assert!(cold_events(&w, &v).is_empty());
}

#[test]
fn a_box_with_no_owning_user_records_why_it_is_cold() {
    let w = world(FakeDriver::new().cache_volumes());
    script_repo(&w, None);
    let v = run(&w.cx, &repo_req("demo")).unwrap();
    assert!(cold_events(&w, &v).is_empty());
    let store = w.cx.store().unwrap();
    let mut row = store.box_row(BoxId::new(v.boxed.id)).unwrap().unwrap();
    row.user = None;
    let cfg =
        crate::ops::common::facts::settings_of(&w.cx, &row, &moor_core::config::Flags::default())
            .unwrap();
    let provider = moor_core::ProviderId::new("local-container");
    let caps = w.cx.driver().caps(&provider).unwrap();
    let plan = crate::ops::common::cache::for_start(&w.cx, store, &row, &cfg, &provider, caps);
    assert!(plan.is_empty());
    assert_eq!(reasons(&w, &v), ["no_user"]);
}
