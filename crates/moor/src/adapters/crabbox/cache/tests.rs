use std::sync::Arc;

use moor_core::cache::{CacheLayers, CachePlan, CacheSalt};
use moor_core::config::{parse_repo, parse_user};
use moor_core::name::ForgeRepo;
use moor_core::seam::driver::Driver;
use moor_core::seam::proc::Tool;
use moor_core::user::UserId;

use super::env_value;
use crate::adapters::crabbox::tests::{req, WARMUP_OUT};
use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::{Reply, ScriptedProc};

fn plan(repo_yaml: &str, user_yaml: &str) -> CachePlan {
    let layers = CacheLayers {
        repo: parse_repo(".moor.yaml", repo_yaml).unwrap().cache,
        operator: parse_user("config.yaml", user_yaml).unwrap().cache,
    };
    let repo = ForgeRepo {
        owner: "o".to_owned(),
        name: "r".to_owned(),
    };
    CachePlan::build(
        UserId::new(1),
        Some(&repo),
        &layers,
        &CacheSalt::new([3; 64]),
    )
    .unwrap()
}

#[test]
fn the_env_value_is_the_wire_contract_exactly() {
    let p = plan(
        "cache: {presets: [npm, yarn, pnpm, bun, docker, podman, dart, rust, go], volumes: [{name: android-sdk, path: /opt/android-sdk, size_gb: 12}]}",
        "",
    );
    let (gradle, sdk) = (p.volumes().first().unwrap(), p.volumes().get(1).unwrap());
    let want = serde_json::json!([
        {"group": gradle.group, "name": "gradle", "key": format!("{}-gradle", gradle.group),
         "paths": ["~/.gradle/caches", "~/.gradle/wrapper"], "sizeGB": 0},
        {"group": sdk.group, "name": "android-sdk", "key": format!("{}-android-sdk", sdk.group),
         "paths": ["/opt/android-sdk"], "sizeGB": 12},
    ]);
    let got = env_value(&p).unwrap();
    assert!(got.starts_with('['), "crabbox reads a JSON array: {got}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&got).unwrap(),
        want
    );
    let fields: Vec<String> =
        serde_json::from_str::<Vec<serde_json::Map<String, serde_json::Value>>>(&got)
            .unwrap()
            .first()
            .unwrap()
            .keys()
            .cloned()
            .collect();
    assert_eq!(
        fields,
        ["group", "name", "key", "paths", "sizeGB"],
        "the contract order; never a seal"
    );
}

#[test]
fn a_volume_with_a_seal_is_sent_sealed_in_a_group_of_its_own() {
    let p = plan(
        "cache: {presets: [yarn, pnpm, bun, podman, gradle, dart, rust, go]}",
        "",
    );
    let npm = p.volumes().first().unwrap();
    let got: serde_json::Value = serde_json::from_str(&env_value(&p).unwrap()).unwrap();
    let group = format!("{}x", npm.group);
    let want = serde_json::json!([
        {"group": npm.group, "name": "npm", "key": format!("{}-npm", npm.group),
         "paths": ["~/.cache/npm"], "sizeGB": 0},
        {"group": group, "name": "docker", "key": format!("{group}-docker"),
         "paths": ["~/docker", "/var/lib/containerd"], "sizeGB": 0, "sealed": true},
    ]);
    assert_eq!(got, want);
}

#[test]
fn a_cold_plan_has_no_env_value() {
    assert_eq!(env_value(&CachePlan::none()), None);
}

#[test]
fn only_warmup_carries_the_volumes() {
    let p = Arc::new(ScriptedProc::new());
    let c = Crabbox::fake(p.clone());
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(WARMUP_OUT.to_owned()));
    p.on(Tool::Crabbox, &["stop"], Reply::Ok(String::new()));
    let mut r = req("external", None);
    r.cache = plan("", "");
    let got = c.acquire(&r).unwrap();
    c.release(&got.handle).unwrap();
    let carried: Vec<(String, bool)> = p
        .calls()
        .iter()
        .map(|call| {
            let has = call
                .env
                .iter()
                .any(|(k, v)| k == "CRABBOX_CACHE_VOLUMES" && v.starts_with('['));
            (call.args.first().cloned().unwrap_or_default(), has)
        })
        .collect();
    assert!(
        carried.contains(&("warmup".to_owned(), true)),
        "{carried:?}"
    );
    assert!(
        carried.iter().all(|(op, has)| op == "warmup" || !has),
        "{carried:?}"
    );
}
