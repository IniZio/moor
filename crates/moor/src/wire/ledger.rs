//! The ledger socket's connections, served against the controller's shared store.

use std::io::{Read, Write};

use moor_core::seam::clock::Ts;
use moor_core::seam::forge::ForgeRepo;
use moor_core::Secret;

use crate::adapters::github::app::AppClient;
use crate::env::{Env, EGRESS_KEY};
use crate::proxy::ledger::{serve, AppTokens, Keys};
use crate::store::Store;
use crate::vault::Vault;

/// Serves one connection on the ledger socket or its TLS listener, to the shared egress Proxy;
/// without a shared store it closes at once, so the Proxy sees the controller as unavailable.
/// Any other hello is refused, so a per-Box Proxy an earlier build left running gives up and
/// exits.
pub(crate) fn serve_ledger((env, vault): (&Env, &Vault), stream: impl Read + Write) {
    if let Some(store) = env.shared::<Store>() {
        let keys = Keys {
            egress: egress_key(env),
        };
        let app = env.shared::<AppClient>().map(|a| a as &dyn AppTokens);
        serve((store, vault, app), &keys, stream);
    }
}

impl AppTokens for AppClient {
    fn repo_token(&self, repo: &ForgeRepo, now: Ts) -> Result<Secret, String> {
        Self::repo_token(self, repo, now)
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

    use moor_core::domain::decision::{Decision, NewBox, SpecFields};
    use moor_core::seam::clock::Ts;
    use moor_core::BoxName;

    use super::serve_ledger;
    use crate::env::Env;
    use crate::proxy::ledger::LEDGER;
    use crate::store::Store;
    use crate::testing::clock::FakeClock;
    use crate::testing::cx::test_vault_key;
    use crate::testing::rng::FakeRng;
    use crate::vault::Vault;

    #[test]
    fn a_supervisor_from_an_earlier_build_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1)));
        let store = Store::open(&dir.path().join("moor.db"), clock).unwrap();
        let nt = NewBox {
            name: BoxName::parse_allow_bare("a").unwrap(),
            repo: None,
            spec: SpecFields::default(),
        };
        let id = store.apply(Decision::insert(nt)).unwrap().box_id;
        drop(store.reserve(id, &FakeRng::default()));
        let vault = Vault::load(&test_vault_key(dir.path())).unwrap();
        let env =
            Env::from_pairs([("MOOR_STATE_DIR", dir.path().to_str().unwrap())]).sharing(store);
        let (ours, theirs) = UnixStream::pair().unwrap();
        let served = std::thread::spawn(move || serve_ledger((&env, &vault), theirs));
        let line = format!(
            "{{\"box\":{{\"v\":{LEDGER},\"box_id\":{},\"key\":\"k\"}}}}\n",
            id.get()
        );
        (&ours).write_all(line.as_bytes()).unwrap();
        let mut reply = String::new();
        BufReader::new(&ours).read_line(&mut reply).unwrap();
        assert!(reply.contains("refused"), "{reply}");
        served.join().unwrap();
    }
}
