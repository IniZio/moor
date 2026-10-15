use moor_core::name::ProviderId;
use moor_core::seam::driver::{DriverCaps, DriverError, EgressSupport, SleepKind};

use super::caps_from;

fn entry(p: &str, aliases: &[&str], features: &[&str], life: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "provider": p, "family": p, "aliases": aliases, "kind": "ssh-lease",
        "targets": ["linux"], "features": features, "lifecycle": life,
        "coordinator": "never", "classCatalog": {}, "futureField": {"x": 1},
    })
}

fn json() -> String {
    serde_json::json!([
        entry("local-container", &[], &["ssh"], &["ephemeral"]),
        entry(
            "namespace-devbox",
            &["namespace", "ns"],
            &[
                "ssh",
                "egress-allow",
                "egress-allow-create-only",
                "resume-rotates-host-key"
            ],
            &["pause-resume"]
        ),
        entry(
            "sprites",
            &[],
            &["ssh", "native-sleep", "egress-allow"],
            &["ephemeral"]
        ),
        entry("ssh", &[], &["ssh"], &["static"]),
        entry("pauser", &[], &["pause-resume"], &[]),
    ])
    .to_string()
}

fn caps(id: &str) -> Result<DriverCaps, DriverError> {
    caps_from(&json(), &ProviderId::new(id))
}

fn want(sleep: SleepKind, wake: bool, egress: EgressSupport) -> DriverCaps {
    DriverCaps {
        sleep,
        wake,
        egress,
        reverse_tunnel: true,
        has_workdir: true,
    }
}

fn no_tunnel(mut caps: DriverCaps) -> DriverCaps {
    caps.reverse_tunnel = false;
    caps
}

#[test]
fn table() {
    let cases = [
        (
            "local-container",
            want(SleepKind::None, false, EgressSupport::None),
        ),
        (
            "namespace-devbox",
            want(SleepKind::Suspend, false, EgressSupport::None),
        ),
        (
            "sprites",
            want(SleepKind::Native, true, EgressSupport::None),
        ),
        ("ssh", want(SleepKind::None, false, EgressSupport::None)),
        (
            "pauser",
            no_tunnel(want(SleepKind::Suspend, true, EgressSupport::None)),
        ),
    ];
    for (id, w) in cases {
        assert_eq!(caps(id), Ok(w), "{id}");
    }
}

#[test]
fn alias_resolves_to_canonical_facts() {
    assert_eq!(caps("ns"), caps("namespace-devbox"));
}

#[test]
fn unknown_provider_is_unsupported() {
    assert_eq!(
        caps("nope"),
        Err(DriverError::Unsupported("unknown box provider"))
    );
}

#[test]
fn unreadable_json_fails_without_raw_text() {
    let e = caps_from("not json SECRET", &ProviderId::new("ssh")).unwrap_err();
    assert!(!e.to_string().contains("SECRET"));
    assert!(matches!(e, DriverError::Failed { .. }));
}

#[test]
fn missing_optional_fields_tolerated() {
    let j = r#"[{"provider":"bare"}]"#;
    assert_eq!(
        caps_from(j, &ProviderId::new("bare")),
        Ok(no_tunnel(want(SleepKind::None, false, EgressSupport::None)))
    );
}
