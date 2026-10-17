//! On a macOS Box the Task home is the agent's alone: `nono`, `gh` and the agent command live
//! in the Box user's real login home, where the batch steps and the launch script start.

use super::*;
use crate::adapters::box_agent::nono_release::fake::{
    ready_release_with_gh, release_of, tarball, FakeFetch,
};
use moor_core::seam::agent_profile::ProxyAddr;
use moor_core::Secret;

const LOGIN: &str = "/Users/alice";
const NOT_INSTALLED: i32 = 6;
const GH_NOT_INSTALLED: i32 = 10;

fn proxy_req(s: &FakeSurface, forge: bool) -> LaunchReq {
    let proxy = ProxyAddr {
        box_port: 4567,
        nonce: Secret::new(b"sentinel-9c2e".to_vec()),
        ca_pem: "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".to_owned(),
        box_env: Vec::new(),
        forge,
    };
    LaunchReq {
        proxy: Some(Arc::new(proxy)),
        origin: Some("git@github.com:o/n.git".to_owned()),
        ..req(s)
    }
}

/// A proxy launch on a Mac layout whose Linux Box answers each step with `codes`.
fn launch(
    codes: &[i32],
    forge: bool,
    release: Arc<crate::adapters::box_agent::NonoRelease>,
) -> (Arc<FakeDriver>, Arc<FakeSurface>) {
    let d = driver(
        FakeDriver::new()
            .run_stdouts([PRINTED])
            .exec_results(codes.iter().map(|&code| Ok(ExecOutcome::Exited { code }))),
    );
    d.set_run_stdout("Linux x86_64\n");
    let s = Arc::new(FakeSurface::new("/r"));
    let a = BoxAgent::new(
        d.clone(),
        s.clone(),
        Arc::new(Claude),
        Arc::new(FakeClock::new(Ts::from_millis(0))),
        release,
    );
    a.launch(&proxy_req(&s, forge)).unwrap();
    (d, s)
}

fn put_paths(d: &FakeDriver) -> Vec<String> {
    (d.puts().into_iter())
        .map(|p| p.1.as_str().to_owned())
        .collect()
}

#[test]
fn a_missing_nono_is_uploaded_to_the_login_home_not_the_task_home() {
    let fetch = Arc::new(FakeFetch::new(Ok(tarball(b"the binary"))));
    let release = Arc::new(release_of(&fetch, &tarball(b"the binary")));
    let (d, _) = launch(&[0, 0, 0, NOT_INSTALLED, 0], false, release);
    let puts = put_paths(&d);
    assert!(
        puts.contains(&format!("{LOGIN}/.local/bin/nono")),
        "{puts:?}"
    );
    assert!(
        !puts
            .iter()
            .any(|p| p.starts_with(HOME) && p.ends_with("/nono")),
        "{puts:?}"
    );
}

#[test]
fn the_sandbox_launcher_is_the_login_homes_nono() {
    let (_, s) = launch(
        &[],
        false,
        crate::adapters::box_agent::nono_release::fake::ready_release(),
    );
    let pane = format!("{:?}", s.opened());
    assert!(pane.contains(&format!("{LOGIN}/.local/bin/nono")), "{pane}");
    assert!(!pane.contains(&format!("{HOME}/.local/bin")), "{pane}");
}

#[test]
fn a_missing_gh_is_uploaded_to_the_login_home_not_the_task_home() {
    let (d, _) = launch(
        &[0, 0, 0, 0, GH_NOT_INSTALLED],
        true,
        ready_release_with_gh(),
    );
    let puts = put_paths(&d);
    assert!(puts.contains(&format!("{LOGIN}/.local/bin/gh")), "{puts:?}");
    assert!(
        !puts
            .iter()
            .any(|p| p.starts_with(HOME) && p.ends_with("/gh")),
        "{puts:?}"
    );
}

#[test]
fn the_agent_path_is_set_before_the_launch_exports_the_task_home() {
    let (_, s) = launch(
        &[],
        false,
        crate::adapters::box_agent::nono_release::fake::ready_release(),
    );
    let pane = format!("{:?}", s.opened());
    let path = pane.find("export PATH=").expect(&pane);
    let home = pane.find("HOME=").expect(&pane);
    assert!(path < home, "{pane}");
}

#[test]
fn a_mac_proxy_launch_leaves_the_task_home_to_the_profile() {
    let (d, s) = launch(&[], false, ready_release_with_gh());
    let script = super::super::script_of(&s.opened().remove(0).argv.pop().unwrap());
    let harden = script
        .find("export XDG_CONFIG_HOME=\"$HOME/.config\"")
        .expect(&script);
    assert!(
        harden < script.find("'--profile'").expect(&script),
        "{script}"
    );
    assert_eq!(count(&script, "export HOME="), 0, "{script}");
    assert_eq!(count(&script, "export XDG_"), 1, "{script}");
    let profile = (d.puts().into_iter())
        .find(|p| p.1.as_str().ends_with("nono-profile.json"))
        .unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&profile.2).unwrap();
    let vars = doc.pointer("/environment/set_vars").expect("set_vars");
    assert_eq!(vars.get("HOME"), Some(&serde_json::Value::from(HOME)));
    assert_eq!(
        vars.get("CFFIXED_USER_HOME"),
        Some(&serde_json::Value::from(HOME))
    );
}

#[test]
fn a_mac_proxy_launch_puts_the_launch_bin_on_path_without_exporting_the_home() {
    let (_, s) = launch(&[], false, ready_release_with_gh());
    let script = super::super::script_of(&s.opened().remove(0).argv.pop().unwrap());
    let line = "PATH='/Users/box/boxes/t1/launch/bin':/opt/homebrew/bin:/opt/homebrew/sbin:\"$PATH\"; export PATH";
    let at = script.find(line).expect(&script);
    let dev = script.find("DEVELOPER_DIR=$(cat").expect(&script);
    assert!(dev < at, "{script}");
    assert!(at < script.find("'--profile'").expect(&script), "{script}");
    assert_eq!(count(&script, "export HOME="), 0, "{script}");
}

#[test]
fn the_profile_passes_the_prepended_path_through_instead_of_setting_it() {
    let (d, _) = launch(&[], false, ready_release_with_gh());
    let profile = (d.puts().into_iter())
        .find(|p| p.1.as_str().ends_with("nono-profile.json"))
        .unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&profile.2).unwrap();
    let set = doc.pointer("/environment/set_vars").expect("set_vars");
    assert_eq!(set.get("PATH"), None, "{doc}");
    let allow = doc.pointer("/environment/allow_vars").expect("allow_vars");
    assert!(allow.as_array().unwrap().contains(&"PATH".into()), "{doc}");
}
