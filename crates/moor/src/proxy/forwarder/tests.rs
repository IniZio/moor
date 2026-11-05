//! Tests for the egress forwarder route and delivery.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::seam::proxy_host::ProxyEndpoint;
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath, ProviderId};

use super::{deliver, Delivery};
use crate::env::Env;
use crate::proxy::route::EgressRoute;
use crate::proxy::token;
use crate::store::{token_hash, Store};
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::rng::FakeRng;

#[test]
fn boxes_reach_the_controllers_own_proxy_unless_an_address_names_another() {
    let route = |pairs: &[(&str, &str)]| EgressRoute::from_env(&Env::from_pairs(pairs.to_vec()));
    assert_eq!(
        route(&[("MOOR_STATE_DIR", "/s")]),
        Ok(EgressRoute::Local {
            socket: "/s/egress/egress.sock".into()
        })
    );
    assert_eq!(
        route(&[
            ("MOOR_STATE_DIR", "/s"),
            ("MOOR_EGRESS_URL", "wss://moor.example/egress")
        ]),
        Ok(EgressRoute::Direct {
            url: "wss://moor.example/egress".to_owned()
        })
    );
}

#[test]
fn a_box_dials_its_own_tunnel_port_on_the_local_route_and_only_wss_elsewhere() {
    let local = EgressRoute::Local {
        socket: "/s/egress/egress.sock".into(),
    };
    assert_eq!(local.upstream(20555), "ws://127.0.0.1:20555/egress");
    assert_eq!(local.socket(), Some(Path::new("/s/egress/egress.sock")));
    let direct = EgressRoute::parse("wss://moor.example/egress").unwrap();
    assert_eq!(direct.upstream(20555), "wss://moor.example/egress");
    assert_eq!(direct.socket(), None);
    for bad in [
        "ws://127.0.0.1:8090/egress",
        "ws://moor.example/egress",
        "https://moor.example/egress",
        "wss://",
        "moor.example",
    ] {
        assert!(EgressRoute::parse(bad).is_err(), "{bad}");
    }
}

struct Rig {
    store: Store,
    driver: FakeDriver,
    handle: BoxHandle,
    id: BoxId,
    binary: tempfile::NamedTempFile,
    _dir: tempfile::TempDir,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = crate::store::db::testing::open_store(&dir.path().join("moor.db"), clock);
    let id = store
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare("t").unwrap(),
            repo: None,
        }))
        .unwrap()
        .box_id;
    let _ = store
        .reserve(id, &FakeRng::default(), Duration::from_secs(30))
        .unwrap();
    let driver = FakeDriver::new();
    let handle = driver
        .acquire(&AcquireReq {
            pool_machine: None,
            provider: ProviderId::new("p"),
            box_id: id,
            name: BoxName::parse_allow_bare("t").unwrap(),
            class: None,
            nonce: "n".to_owned(),
            cache: moor_core::cache::CachePlan::none(),
        })
        .unwrap()
        .handle;
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    driver
        .put_file(&handle, &environ, b"PATH=/bin\0HOME=/home/box\0", 0o400)
        .unwrap();
    let binary = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(binary.path(), b"forwarder-bytes").unwrap();
    Rig {
        store,
        driver,
        handle,
        id,
        binary,
        _dir: dir,
    }
}

impl Rig {
    fn endpoint() -> ProxyEndpoint {
        ProxyEndpoint {
            box_port: 20555,
            egress_port: 10555,
            forward_port: Some(10556),
            ca_pem: String::new(),
        }
    }

    fn deliver(&self, route: &EgressRoute, ep: &ProxyEndpoint) -> Result<u16, String> {
        self.deliver_from(route, ep, self.binary.path())
    }

    fn deliver_from(
        &self,
        route: &EgressRoute,
        ep: &ProxyEndpoint,
        binary: &Path,
    ) -> Result<u16, String> {
        self.deliver_under(route, ep, binary, KEY)
    }

    fn deliver_under(
        &self,
        route: &EgressRoute,
        ep: &ProxyEndpoint,
        binary: &Path,
        key: &[u8],
    ) -> Result<u16, String> {
        deliver(
            &Delivery {
                route,
                key,
                driver: &self.driver,
                store: &self.store,
                box_id: self.id,
                handle: Some(&self.handle),
                binary,
            },
            ep,
        )
    }

    /// The forwarder files written into the Box: path, bytes and mode.
    fn written(&self) -> Vec<(String, Vec<u8>, u32)> {
        self.driver
            .puts()
            .into_iter()
            .filter(|(_, path, _, _)| path.as_str().starts_with("/home/box/"))
            .map(|(_, path, bytes, mode)| (path.as_str().to_owned(), bytes, mode))
            .collect()
    }
}

const KEY: &[u8] = b"egress-key";

fn tunneled() -> EgressRoute {
    EgressRoute::Local {
        socket: "/s/egress/egress.sock".into(),
    }
}

#[test]
fn the_first_delivery_mints_a_token_and_writes_the_forwarder_then_its_config() {
    let r = rig();
    assert_eq!(r.deliver(&tunneled(), &Rig::endpoint()), Ok(10556));
    let written = r.written();
    let [bin, config] = written.as_slice() else {
        unreachable!("two files, not {written:?}")
    };
    assert_eq!(
        (bin.0.as_str(), bin.1.as_slice(), bin.2),
        (
            "/home/box/.moor/egress-forward.new",
            b"forwarder-bytes".as_slice(),
            0o755
        )
    );
    assert_eq!(config.0, "/home/box/.moor/egress-forward.json");
    assert_eq!(config.2, 0o600, "only the Box user reads the token");
    let config: serde_json::Value = serde_json::from_slice(&config.1).unwrap();
    assert_eq!(config.get("listen").unwrap(), "127.0.0.1:10556");
    assert_eq!(
        config.get("upstream").unwrap(),
        "ws://127.0.0.1:20555/egress"
    );
    let sent = config.get("token").unwrap().as_str().unwrap();
    assert!(
        token::verify(KEY, sent.as_bytes()),
        "the shared Proxy accepts it"
    );
    assert_eq!(
        r.store
            .egress_token_box(&token_hash(sent.as_bytes()))
            .unwrap(),
        Some(r.id)
    );
    assert!(r
        .store
        .egress_token_delivered(r.id, token::key_id(KEY))
        .unwrap());
}

#[test]
fn a_delivered_token_is_not_delivered_again() {
    let r = rig();
    r.deliver(&tunneled(), &Rig::endpoint()).unwrap();
    assert_eq!(r.deliver(&tunneled(), &Rig::endpoint()), Ok(10556));
    assert_eq!(r.written().len(), 2, "one delivery");
    assert!(
        r.driver.execs().is_empty(),
        "delivery runs no command of its own"
    );
}

#[test]
fn a_new_egress_key_delivers_a_token_under_it() {
    let r = rig();
    r.deliver(&tunneled(), &Rig::endpoint()).unwrap();
    let other = b"another-egress-key";
    let ep = Rig::endpoint();
    assert_eq!(
        r.deliver_under(&tunneled(), &ep, r.binary.path(), other),
        Ok(10556)
    );
    let written = r.written();
    assert_eq!(written.len(), 4, "delivered again");
    let config: serde_json::Value = serde_json::from_slice(&written.get(3).unwrap().1).unwrap();
    let sent = config.get("token").unwrap().as_str().unwrap();
    assert!(token::verify(other, sent.as_bytes()), "under the new key");
    assert!(!r
        .store
        .egress_token_delivered(r.id, token::key_id(KEY))
        .unwrap());
}

#[test]
fn a_failed_write_leaves_the_token_undelivered_for_the_next_exec() {
    let r = rig();
    let r = Rig {
        driver: r
            .driver
            .fail_put(moor_core::seam::driver::DriverError::Unavailable(
                "down".to_owned(),
            )),
        ..r
    };
    assert!(r.deliver(&tunneled(), &Rig::endpoint()).is_err());
    assert!(!r
        .store
        .egress_token_delivered(r.id, token::key_id(KEY))
        .unwrap());
    assert_eq!(r.deliver(&tunneled(), &Rig::endpoint()), Ok(10556));
    assert!(r
        .store
        .egress_token_delivered(r.id, token::key_id(KEY))
        .unwrap());
}

#[test]
fn a_proxy_from_before_the_forwarder_is_refused_on_the_shared_route() {
    let r = rig();
    let mut ep = Rig::endpoint();
    ep.forward_port = None;
    let err = r.deliver(&tunneled(), &ep).unwrap_err();
    assert!(err.contains("start the Box again"), "{err}");
}

#[test]
fn a_missing_binary_is_named_and_nothing_is_minted() {
    let r = rig();
    let gone = Path::new("/nonexistent/moor-egress-forward");
    let err = r
        .deliver_from(&tunneled(), &Rig::endpoint(), gone)
        .unwrap_err();
    assert!(err.contains("/nonexistent/moor-egress-forward"), "{err}");
    assert!(!r
        .store
        .egress_token_delivered(r.id, token::key_id(KEY))
        .unwrap());
}

#[test]
fn the_controller_delivers_the_forwarder_installed_beside_moor() {
    let exe = std::env::current_exe().unwrap();
    assert_eq!(
        crate::proxy::route::binary(&Env::from_pairs(Vec::<(&str, &str)>::new())),
        exe.with_file_name("moor-egress-forward")
    );
    assert_eq!(
        crate::proxy::route::binary(&Env::from_pairs([("MOOR_EGRESS_FORWARD_BIN", "/opt/fwd")])),
        Path::new("/opt/fwd")
    );
}

#[test]
fn a_mark_lost_to_another_controllers_mint_is_tried_again() {
    let r = rig();
    let writes = std::cell::Cell::new(0);
    let other = |_: &str| {
        writes.set(writes.get() + 1);
        if writes.get() == 1 {
            token::mint(&r.store, KEY, r.id).unwrap();
        }
        Ok(())
    };
    super::mint_until_marked(&r.store, KEY, r.id, other).unwrap();
    assert_eq!(writes.get(), 2);
    assert!(r
        .store
        .egress_token_delivered(r.id, token::key_id(KEY))
        .unwrap());
}

#[test]
fn every_mark_lost_fails_the_delivery() {
    let r = rig();
    let writes = std::cell::Cell::new(0);
    let other = |_: &str| {
        writes.set(writes.get() + 1);
        token::mint(&r.store, KEY, r.id).unwrap();
        Ok(())
    };
    let err = super::mint_until_marked(&r.store, KEY, r.id, other).unwrap_err();
    assert!(err.contains("another controller replaced"), "{err}");
    assert_eq!(writes.get(), super::MINT_TRIES);
    assert!(!r
        .store
        .egress_token_delivered(r.id, token::key_id(KEY))
        .unwrap());
}
