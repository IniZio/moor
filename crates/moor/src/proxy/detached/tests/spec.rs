//! The spec the adapter records for the supervisor, and the CA certificate it hands the Box.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use moor_core::config::proxy::{ResolvedInject, ResolvedRule};
use moor_core::seam::proxy_host::{ProxyHost, ProxySpec};

use super::{bare, fx, supervise, Fx};
use crate::store::{Store, STORE_FILE};
use crate::testing::clock::FakeClock;

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
fn ensure_records_the_spec_before_it_spawns() {
    let f = bare();
    let seen = Arc::new(AtomicBool::new(false));
    let state = f.dir.path().to_path_buf();
    let (flag, id) = (seen.clone(), f.id);
    let inner = supervise(state.clone());
    f.proc.on_daemon(move |d| {
        let clock = Arc::new(FakeClock::new(moor_core::seam::clock::Ts::from_millis(
            1000,
        )));
        let store = Store::open(&state.join(STORE_FILE), clock).unwrap();
        flag.store(store.proxy_spec(id).unwrap().is_some(), Ordering::SeqCst);
        inner(d);
    });
    let spec = spec_with_rule(&f);

    f.sup.ensure(&spec).unwrap();

    assert!(seen.load(Ordering::SeqCst));
    assert_eq!(f.store.proxy_spec(f.id).unwrap(), Some(spec));
}

#[test]
fn a_reused_supervisor_still_gets_the_latest_spec() {
    let f = fx();
    f.sup.ensure(&f.spec()).unwrap();
    let newer = spec_with_rule(&f);

    f.sup.ensure(&newer).unwrap();

    assert_eq!(f.proc.daemons().len(), 1);
    assert_eq!(f.store.proxy_spec(f.id).unwrap(), Some(newer));
}

#[test]
fn the_endpoint_carries_the_ca_certificate_the_row_keeps() {
    let f = fx();

    let first = f.sup.ensure(&f.spec()).unwrap();
    let second = f.sup.ensure(&f.spec()).unwrap();

    let kept = f.store.proxy_ca(f.id).unwrap().unwrap();
    assert!(first.ca_pem.starts_with("-----BEGIN CERTIFICATE-----"));
    assert_eq!(first.ca_pem, kept.cert_pem);
    assert_eq!(second.ca_pem, kept.cert_pem);
}
