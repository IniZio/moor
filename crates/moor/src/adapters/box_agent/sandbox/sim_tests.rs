use moor_core::config::RepoSandbox;
use moor_core::exec_env::{self, BoxCtx, MacTask, Purpose};
use moor_core::name::{BoxPath, TaskName};
use moor_core::task_key::{MacLayout, TaskKey};
use serde_json::Value;

use super::{mac::MacInput, profile, SandboxInput};

fn path(s: &str) -> BoxPath {
    BoxPath::restore(s).unwrap()
}

fn at<'a>(v: &'a Value, pointer: &str) -> &'a Value {
    v.pointer(pointer).unwrap_or(&Value::Null)
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_owned())
        .collect()
}

/// The profile a Mac Task gets for `sim_udid`, built from the same exec environment every exec
/// uses.
fn sim_doc(sim_udid: Option<&str>) -> Value {
    let (w, h, c) = (
        path("/Users/Shared/moor/t/work"),
        path("/Users/Shared/moor/t/home"),
        path("/tmp/cred"),
    );
    let tmp = path("/Users/Shared/moor/t/tmp");
    let key = TaskKey::new(&TaskName::parse("t").unwrap());
    let layout = MacLayout::new(&path("/Users/Shared/moor"), &key).unwrap();
    let (login_home, sandbox) = (path("/Users/newman"), RepoSandbox::default());
    let task = MacTask {
        layout: &layout,
        key: &key,
        nonce: "n",
        toolchain_env: &[],
        toolchain_bin: &[],
        sim_udid,
    };
    let ctx = BoxCtx {
        env: &[],
        proxy: None,
        origin: None,
        workdir: None,
        strip: &[],
        mac: Some(task),
    };
    let env = exec_env::build(&ctx, Purpose::Agent).unwrap();
    let input = SandboxInput {
        os: "Darwin",
        workdir: &w,
        home: &h,
        cred: &c,
        tmp: &tmp,
        env_names: vec![],
        set_vars: env.vars,
        root: false,
        strip_env: &[],
        mac: Some(MacInput {
            layout: &layout,
            login_home: &login_home,
            developer_dir: "/Applications/Xcode.app/Contents/Developer",
            capabilities: &[],
            sandbox: &sandbox,
            toolchains: &[],
        }),
    };
    serde_json::from_str(&profile(&input).unwrap()).unwrap()
}

#[test]
fn a_task_with_a_simulator_sets_its_names_in_the_profile() {
    let d = sim_doc(Some("UDID-1"));
    assert_eq!(at(&d, "/environment/set_vars/MOOR_SIM_UDID"), "UDID-1");
    assert_eq!(
        at(&d, "/environment/set_vars/MOOR_SIM_SET"),
        "/Users/Shared/moor/t/sim"
    );
}

#[test]
fn a_task_without_a_simulator_gets_no_simulator_names() {
    // A stale value in the Box login environment must not be passed through or set.
    let d = sim_doc(None);
    let allowed = strings(at(&d, "/environment/allow_vars"));
    let set = at(&d, "/environment/set_vars");
    for name in ["MOOR_SIM_UDID", "MOOR_SIM_SET"] {
        assert!(!allowed.iter().any(|a| a == name), "{name} allowed");
        assert!(set.get(name).is_none(), "{name} set");
    }
}
