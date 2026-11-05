//! The ledger socket's connections, served against the controller's shared store.

use std::io::{Read, Write};

use crate::env::{Env, EGRESS_KEY};
use crate::proxy::ledger::{serve, Keys};
use crate::store::Store;
use crate::vault::Vault;

/// Serves one connection on the ledger socket, to the front and shared egress Proxies; without a
/// shared store it closes at once, so the Proxy sees the controller as unavailable. A Box
/// supervisor an earlier build started is refused, so it gives up its lease and exits.
pub(crate) fn serve_ledger(
    (env, vault): (&Env, &Vault),
    fronts_key: Option<&str>,
    stream: impl Read + Write,
) {
    if let Some(store) = env.shared::<Store>() {
        let keys = Keys {
            supervisors: false,
            fronts: fronts_key,
            egress: egress_key(env),
        };
        serve((store, vault), &keys, stream);
    }
}

/// Serves one connection on the ledger's TLS listener, to the shared egress Proxy only.
pub(crate) fn serve_egress_ledger((env, vault): (&Env, &Vault), stream: impl Read + Write) {
    if let Some(store) = env.shared::<Store>() {
        let keys = Keys {
            supervisors: false,
            fronts: None,
            egress: egress_key(env),
        };
        serve((store, vault), &keys, stream);
    }
}

/// The shared egress Proxy's key; none while it is off.
fn egress_key(env: &Env) -> Option<&str> {
    env.get(EGRESS_KEY).filter(|k| !k.is_empty())
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;
    use std::time::Duration;

    use moor_core::domain::decision::{Decision, NewBox};
    use moor_core::seam::clock::Ts;
    use moor_core::BoxName;

    use super::serve_ledger;
    use crate::env::Env;
    use crate::proxy::ledger::{Hello, LEDGER};
    use crate::store::Store;
    use crate::testing::clock::FakeClock;
    use crate::testing::cx::test_vault_key;
    use crate::testing::rng::FakeRng;
    use crate::vault::Vault;

    #[test]
    fn a_supervisor_from_an_earlier_build_is_refused_even_with_its_key() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1)));
        let store = Store::open(&dir.path().join("moor.db"), clock).unwrap();
        let nt = NewBox {
            name: BoxName::parse_allow_bare("a").unwrap(),
            repo: None,
        };
        let id = store.apply(Decision::insert(nt)).unwrap().box_id;
        drop(store.reserve(id, &FakeRng::default(), Duration::from_secs(30)));
        store.set_proxy_key(id, b"k", LEDGER).unwrap();
        let vault = Vault::load(&test_vault_key(dir.path())).unwrap();
        let env =
            Env::from_pairs([("MOOR_STATE_DIR", dir.path().to_str().unwrap())]).sharing(store);
        let (ours, theirs) = UnixStream::pair().unwrap();
        let served = std::thread::spawn(move || serve_ledger((&env, &vault), None, theirs));
        let hello = Hello::Box {
            v: LEDGER,
            box_id: id,
            key: "k".to_owned(),
        };
        let mut line = serde_json::to_string(&hello).unwrap();
        line.push('\n');
        (&ours).write_all(line.as_bytes()).unwrap();
        let mut reply = String::new();
        BufReader::new(&ours).read_line(&mut reply).unwrap();
        assert!(reply.contains("refused"), "{reply}");
        served.join().unwrap();
    }
}
