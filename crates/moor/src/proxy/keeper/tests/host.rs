//! The controller's Proxy host: a Box's proxy step records its row and delivers its forwarder,
//! and refuses while the shared egress Proxy on this host is down.

use std::os::unix::net::UnixListener;
use std::sync::Arc;

use moor_core::seam::clock::Ts;
use moor_core::seam::driver::Driver;

use super::super::{Deps, Keepers, KeptProxy};
use super::World;
use crate::proxy::links::Runners;
use crate::testing::clock::FakeClock;
use crate::testing::rng::FakeRng;

/// The controller's Proxy host over `w`, delivering a stand-in forwarder for the direct route.
fn kept_proxy(w: &World) -> KeptProxy {
    let binary = w.dir.path().join("moor-egress-forward");
    std::fs::write(&binary, b"forwarder-bytes").unwrap();
    let deps = Deps {
        store: w.store.clone(),
        driver: Arc::clone(&w.driver) as Arc<dyn Driver>,
        clock: Arc::new(FakeClock::new(Ts::from_millis(1_000))),
        egress: None,
        runners: Runners::new(
            w.dir.path(),
            None,
            Arc::new(FakeClock::new(Ts::from_millis(1_000))),
            None,
        ),
        log: w.log.clone(),
    };
    KeptProxy {
        deps: Arc::new(deps),
        rng: Arc::new(FakeRng::default()),
        vault_key: Some(crate::testing::cx::test_vault_key(w.dir.path())),
        state_dir: None,
        keepers: Box::leak(Box::<Keepers>::default()),
        route: crate::proxy::route::EgressRoute::parse("wss://egress.test/egress").unwrap(),
        key: zeroize::Zeroizing::new(b"egress-key".to_vec()),
        binary,
    }
}

#[test]
fn a_row_from_before_the_forwarder_gets_a_forward_port_and_the_exec_delivers_it() {
    use moor_core::seam::proxy_host::{ProxyHost as _, ProxySpec};
    use moor_core::BoxPath;
    use moor_schema::tables::Proxy;
    use sea_query::{Expr, ExprTrait as _, Query};

    use crate::store::db::DbExt as _;

    let w = World::new([]);
    let before = w.store.proxy(w.id).unwrap().unwrap();
    w.store
        .writer()
        .run(
            Query::update()
                .table(Proxy::Table)
                .value(Proxy::ForwardPort, Option::<i64>::None)
                .and_where(Expr::col(Proxy::Box).eq(w.id.get())),
        )
        .unwrap();
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    w.driver
        .put_file(&w.handle(), &environ, b"HOME=/home/box\0", 0o400)
        .unwrap();
    let host = kept_proxy(&w);
    let spec = ProxySpec {
        box_id: w.id,
        box_handle: Some(w.handle()),
        box_repo: None,
        other_repos: Vec::new(),
        rules: Vec::new(),
        user: moor_core::user::UserId::new(1),
        hands_inc: None,
    };
    let ep = host.ensure(&spec).unwrap();
    let after = w.store.proxy(w.id).unwrap().unwrap();
    assert!(
        after.forward_port.is_some(),
        "the old row got a forward port"
    );
    assert_eq!(ep.forward_port, after.forward_port);
    assert_eq!(
        (after.box_port, after.egress_port),
        (before.box_port, before.egress_port),
        "its other ports are kept"
    );
    let delivered: Vec<String> = w
        .driver
        .puts()
        .into_iter()
        .map(|(_, path, _, _)| path.as_str().to_owned())
        .collect();
    assert!(
        delivered.contains(&"/home/box/.moor/egress-forward.json".to_owned()),
        "{delivered:?}"
    );
}

#[test]
fn a_box_whose_local_shared_proxy_has_stopped_fails_its_proxy_step_naming_it() {
    use moor_core::seam::proxy_host::{ProxyHost as _, ProxyHostError, ProxySpec};

    let w = World::new([]);
    let environ = moor_core::BoxPath::restore("/proc/self/environ").unwrap();
    w.driver
        .put_file(&w.handle(), &environ, b"HOME=/home/box\0", 0o400)
        .unwrap();
    let socket = w.socket();
    drop(UnixListener::bind(&socket).unwrap());
    let mut host = kept_proxy(&w);
    host.deps = Arc::new(Deps {
        store: w.store.clone(),
        driver: Arc::clone(&w.driver) as Arc<dyn Driver>,
        clock: Arc::new(FakeClock::new(Ts::from_millis(1_000))),
        egress: Some(socket.clone()),
        runners: Runners::new(
            w.dir.path(),
            None,
            Arc::new(FakeClock::new(Ts::from_millis(1_000))),
            None,
        ),
        log: w.log.clone(),
    });
    host.route = crate::proxy::route::EgressRoute::Local { socket };
    let spec = ProxySpec {
        box_id: w.id,
        box_handle: Some(w.handle()),
        box_repo: None,
        other_repos: Vec::new(),
        rules: Vec::new(),
        user: moor_core::user::UserId::new(1),
        hands_inc: None,
    };
    let Err(ProxyHostError::Failed(why)) = host.ensure(&spec) else {
        unreachable!("the proxy step went on without the shared egress Proxy")
    };
    assert!(why.contains("egress Proxy is not running"), "{why}");
    assert!(
        w.driver.tunnels().is_empty(),
        "no tunnel leads to a dead Proxy"
    );
}
