//! Each Box's keeper in this controller, and the Proxy host that starts them.

use std::path::Path;
use std::sync::{Arc, Mutex};

use moor_core::seam::clock::Clock;
use moor_core::seam::driver::Driver;
use moor_core::seam::proxy_host::ProxyHost;
use moor_core::seam::rng::Rng;
use zeroize::Zeroizing;

use super::parts;
use crate::adapters::sockets::socket_dir;
use crate::env::Env;
use crate::proxy::keeper::{Deps, KeptProxy, NoProxyHost, KEEPERS};
use crate::proxy::links::Runners;
use crate::proxy::route::{self, EgressRoute};
use crate::store::Store;

/// Starts a keeper for each proxy row in the store `env` shares; its first tick opens the Box's
/// tunnel and forwards, or lets them go when the Box closed. One line per failure.
pub(crate) fn reconcile_proxies(env: &Env) -> Vec<String> {
    let Some(store) = env.shared::<Store>() else {
        return Vec::new();
    };
    let (parts, _) = match parts(env, socket_dir()) {
        Ok(parts) => parts,
        Err(e) => return vec![e.error.to_string()],
    };
    let Some(deps) = keeper_deps(env, &parts.driver, &parts.clock) else {
        return Vec::new();
    };
    let rows = match store.proxies() {
        Ok(rows) => rows,
        Err(e) => return vec![format!("cannot list the proxy rows: {e}")],
    };
    for row in rows {
        KEEPERS.start(&deps, row.box_id, row.box_port);
    }
    Vec::new()
}

/// Whether the keepers in `env` tunnel into each Box: only to reach a shared egress Proxy on a
/// socket of this host.
pub(crate) fn keepers_tunnel(env: &Env) -> bool {
    EgressRoute::from_env(env).is_ok_and(|r| r.socket().is_some())
}

/// Ends every Box's keeper in this process.
pub(crate) fn end_keepers() {
    KEEPERS.end_all();
}

/// What each Box's keeper uses, in a controller; `None` without a shared store.
fn keeper_deps(env: &Env, driver: &Arc<dyn Driver>, clock: &Arc<dyn Clock>) -> Option<Arc<Deps>> {
    let store = env.shared::<Store>()?.clone();
    let state_dir = env.state_dir()?;
    let egress = EgressRoute::from_env(env)
        .ok()
        .and_then(|r| r.socket().map(Path::to_path_buf));
    let key = env.get(crate::env::MACHINE_SERVICE_KEY);
    let runners = Runners::new(&state_dir, Some(store.clone()), Arc::clone(clock), key);
    Some(Arc::new(Deps {
        store,
        driver: Arc::clone(driver),
        clock: Arc::clone(clock),
        egress,
        runners,
        log: Arc::new(Mutex::new(std::io::stderr())),
    }))
}

/// The Box Proxies' host: each Box's keeper in this controller, and the shared egress Proxy.
pub(super) fn proxy_host(
    env: &Env,
    driver: &Arc<dyn Driver>,
    clock: &Arc<dyn Clock>,
    rng: &Arc<dyn Rng>,
) -> Arc<dyn ProxyHost> {
    let (Some(deps), Ok(route)) = (keeper_deps(env, driver, clock), EgressRoute::from_env(env))
    else {
        return Arc::new(NoProxyHost);
    };
    Arc::new(KeptProxy {
        deps,
        rng: Arc::clone(rng),
        vault_key: env.vault_key().map(Path::to_path_buf),
        state_dir: env.state_dir(),
        keepers: &KEEPERS,
        route,
        key: Zeroizing::new(route::key(env).to_vec()),
        binary: route::binary(env),
    })
}

#[cfg(test)]
mod tests {
    use super::keepers_tunnel;
    use crate::env::Env;

    #[test]
    fn only_keepers_of_a_local_shared_proxy_tunnel() {
        let local = Env::from_pairs([("MOOR_STATE_DIR", "/s")]);
        assert!(keepers_tunnel(&local));
        let direct = Env::from_pairs([
            ("MOOR_STATE_DIR", "/s"),
            ("MOOR_EGRESS_URL", "wss://egress.test/egress"),
        ]);
        assert!(!keepers_tunnel(&direct));
    }
}
