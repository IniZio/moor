//! The spec file and CA certificate the adapter prepares for the supervisor.

use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use moor_core::config::proxy::{ResolvedInject, ResolvedRule};
use moor_core::seam::proxy_host::{ProxyHost, ProxySpec};

use super::{bare, fx, supervise, Fx};
use crate::proxy::detached::spec::spec_path;
use crate::proxy::detached::{read_spec, write_spec};
use crate::proxy::nono::ca::ca_dir;

fn spec_with_rule(f: &Fx) -> ProxySpec {
    let mut spec = f.spec();
    spec.rules.push(ResolvedRule {
        name: "claude".into(),
        hosts: vec!["api.example.com".into()],
        credential: "claude".into(),
        inject: ResolvedInject::Header {
            header: "authorization".into(),
            form: "Bearer {secret}".into(),
        },
        endpoint_rules: Vec::new(),
        box_env: [("KEY".to_owned(), "moor-proxy".to_owned())].into(),
    });
    spec
}

#[test]
fn a_written_spec_reads_back_equal() {
    let f = fx();
    let spec = spec_with_rule(&f);

    write_spec(f.dir.path(), &spec).unwrap();

    assert_eq!(read_spec(f.dir.path(), f.id).unwrap(), spec);
}

#[test]
fn the_spec_file_is_owner_only_in_an_owner_only_directory() {
    let f = fx();

    write_spec(f.dir.path(), &f.spec()).unwrap();

    let file = std::fs::metadata(spec_path(f.dir.path(), f.id)).unwrap();
    let dir = std::fs::metadata(ca_dir(f.dir.path(), f.id)).unwrap();
    assert_eq!(file.permissions().mode() & 0o777, 0o600);
    assert_eq!(dir.permissions().mode() & 0o777, 0o700);
}

#[test]
fn ensure_writes_the_spec_before_it_spawns() {
    let f = bare();
    let seen = Arc::new(AtomicBool::new(false));
    let state = f.dir.path().to_path_buf();
    let (flag, id) = (seen.clone(), f.id);
    let inner = supervise(state.clone());
    f.proc.on_daemon(move |d| {
        flag.store(read_spec(&state, id).is_ok(), Ordering::SeqCst);
        inner(d);
    });
    let spec = spec_with_rule(&f);

    f.sup.ensure(&spec).unwrap();

    assert!(seen.load(Ordering::SeqCst));
    assert_eq!(read_spec(f.dir.path(), f.id).unwrap(), spec);
}

#[test]
fn a_reused_supervisor_still_gets_the_latest_spec() {
    let f = fx();
    f.sup.ensure(&f.spec()).unwrap();
    let newer = spec_with_rule(&f);

    f.sup.ensure(&newer).unwrap();

    assert_eq!(f.proc.daemons().len(), 1);
    assert_eq!(read_spec(f.dir.path(), f.id).unwrap(), newer);
}

#[test]
fn the_endpoint_carries_one_stable_ca_certificate() {
    let f = fx();

    let first = f.sup.ensure(&f.spec()).unwrap();
    let second = f.sup.ensure(&f.spec()).unwrap();

    assert!(first.ca_pem.starts_with("-----BEGIN CERTIFICATE-----"));
    assert_eq!(first.ca_pem, second.ca_pem);
}
