//! The images check: the Box images the installed agent will pull.

use std::fmt::Write as _;
use std::fs;
use std::sync::Mutex;

use super::*;
use crate::doctor::images::{classify_manifest, Presence};

fn installed(vars: &[(&str, &str)]) -> (Rig, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let units = home.path().join(".config/systemd/user");
    fs::create_dir_all(&units).unwrap();
    let env = vars.iter().fold(String::new(), |mut env, (k, v)| {
        let _ = writeln!(env, "Environment=\"{k}={v}\"");
        env
    });
    fs::write(
        units.join("moor-agent.service"),
        format!("[Service]\nExecStart=\"/opt/moor/moor-machine-agent\" \"run\"\n{env}"),
    )
    .unwrap();
    let mut rig = Rig::new();
    rig.env = Env::from_pairs([("HOME", home.path().to_str().unwrap())]);
    rig.agent_version =
        Box::new(|exe| (exe == "/opt/moor/moor-machine-agent").then(|| "9.9.9".to_owned()));
    (rig, home)
}

fn run(rig: &mut Rig, probe: impl Fn(&str) -> Presence + Sync + 'static) -> Vec<Check> {
    rig.present = Box::new(probe);
    rig.with(images::checks)
}

#[test]
fn a_host_with_no_agent_unit_has_no_images_to_check() {
    let mut rig = Rig::new();
    assert_eq!(run(&mut rig, |_| Presence::Missing), []);
}

#[test]
fn the_default_images_carry_the_agents_own_version() {
    let (mut rig, _home) = installed(&[]);
    let asked = std::sync::Arc::new(Mutex::new(Vec::new()));
    let log = asked.clone();
    let checks = run(&mut rig, move |image| {
        log.lock().unwrap().push(image.to_owned());
        Presence::Local
    });
    let mut asked = asked.lock().unwrap().clone();
    asked.sort();
    assert_eq!(
        asked,
        [
            "ghcr.io/inizio/moor-agent:9.9.9",
            "ghcr.io/inizio/moor-work:9.9.9"
        ]
    );
    assert!(checks.iter().all(|c| c.status == Status::Ok), "{checks:?}");
}

#[test]
fn the_units_own_images_win_over_the_default() {
    let (mut rig, _home) = installed(&[
        (
            "MOOR_AGENT_IMAGE",
            "ghcr.io/inizio/moor-agent:0.3.0-nightly.398",
        ),
        ("MOOR_WORK_IMAGE", "work:dev"),
    ]);
    let asked = std::sync::Arc::new(Mutex::new(Vec::new()));
    let log = asked.clone();
    run(&mut rig, move |image| {
        log.lock().unwrap().push(image.to_owned());
        Presence::Pullable
    });
    let mut asked = asked.lock().unwrap().clone();
    asked.sort();
    assert_eq!(
        asked,
        ["ghcr.io/inizio/moor-agent:0.3.0-nightly.398", "work:dev"]
    );
}

#[test]
fn an_agent_that_will_not_say_its_version_falls_back_to_this_release() {
    let (mut rig, _home) = installed(&[]);
    rig.agent_version = Box::new(|_| None);
    let asked = std::sync::Arc::new(Mutex::new(Vec::new()));
    let log = asked.clone();
    run(&mut rig, move |image| {
        log.lock().unwrap().push(image.to_owned());
        Presence::Local
    });
    let want = format!("ghcr.io/inizio/moor-agent:{}", env!("CARGO_PKG_VERSION"));
    assert!(asked.lock().unwrap().contains(&want));
}

#[test]
fn an_image_that_does_not_exist_fails_the_doctor_and_names_the_fix() {
    let (mut rig, _home) = installed(&[]);
    let checks = run(&mut rig, |_| Presence::Missing);
    assert_eq!(checks.len(), 2);
    for c in &checks {
        assert_eq!((c.name, c.status), ("images", Status::Fail), "{c:?}");
        assert!(c.detail.contains("ghcr.io/inizio/moor-"), "{c:?}");
        assert!(c.detail.contains("9.9.9"), "{c:?}");
        assert!(
            c.fix.contains("set MOOR_AGENT_IMAGE and MOOR_WORK_IMAGE"),
            "{c:?}"
        );
        assert!(c.fix.contains("nightly"), "{c:?}");
    }
}

#[test]
fn up_only_warns_about_an_image_that_does_not_exist() {
    let (mut rig, _home) = installed(&[]);
    rig.images_fail = false;
    let checks = run(&mut rig, |_| Presence::Missing);
    assert!(
        checks.iter().all(|c| c.status == Status::Warn),
        "{checks:?}"
    );
    assert!(
        checks.iter().all(|c| c.fix.contains("MOOR_AGENT_IMAGE")),
        "{checks:?}"
    );
}

#[test]
fn a_registry_that_cannot_be_reached_is_a_warning_even_for_the_doctor() {
    let (mut rig, _home) = installed(&[]);
    let checks = run(&mut rig, |_| {
        Presence::Unknown("dial tcp: no such host".to_owned())
    });
    assert!(
        checks.iter().all(|c| c.status == Status::Warn),
        "{checks:?}"
    );
    assert!(
        checks.iter().all(|c| c.detail.contains("no such host")),
        "{checks:?}"
    );
}

#[test]
fn an_image_on_this_host_or_in_a_registry_passes() {
    let (mut rig, _home) = installed(&[]);
    let checks = run(&mut rig, |image| {
        if image.contains("agent") {
            Presence::Local
        } else {
            Presence::Pullable
        }
    });
    let details: Vec<&str> = checks.iter().map(|c| c.detail.as_str()).collect();
    assert!(details
        .iter()
        .any(|d| d.contains("agent image") && d.contains("on this host")));
    assert!(details
        .iter()
        .any(|d| d.contains("work image") && d.contains("can be pulled")));
    assert!(checks.iter().all(|c| c.fix.is_empty()));
}

#[test]
fn a_registry_s_not_found_answers_are_missing_and_anything_else_is_unknown() {
    for said in [
        "no such manifest: ghcr.io/inizio/moor-agent:0.2.0",
        "manifest unknown",
        "Error response from daemon: manifest for x:1 not found: manifest unknown",
        "name unknown: repository name not known to registry",
    ] {
        assert_eq!(classify_manifest(said), Presence::Missing, "{said}");
    }
    for said in [
        "Get \"https://ghcr.io/v2/\": dial tcp: lookup ghcr.io: no such host",
        "context deadline exceeded",
        "",
    ] {
        assert!(
            matches!(classify_manifest(said), Presence::Unknown(_)),
            "{said}"
        );
    }
}
