//! The Proxy host the controller runs: each Box's credential traffic goes to the shared egress
//! Proxy, and the Box's keeper holds the tunnel and machine forwards it needs.

use std::path::PathBuf;
use std::sync::Arc;

use moor_core::seam::proxy_host::{
    ProxyEndpoint, ProxyHost, ProxyHostError, ProxySpec, ProxyStatus,
};
use moor_core::seam::rng::Rng;
use moor_core::BoxId;

use zeroize::Zeroizing;

use super::{Deps, Keepers};
use crate::proxy::forwarder::{deliver, Delivery};
use crate::proxy::ledger::{box_ca, Fault};
use crate::proxy::route::EgressRoute;
use crate::store::{ProxyRow, StoreError};
use crate::vault::Vault;

/// Runs each Box's keeper in this process and answers for the shared egress Proxy.
pub(crate) struct KeptProxy {
    /// What each keeper uses.
    pub(crate) deps: Arc<Deps>,
    /// Draws the ports of a new proxy row.
    pub(crate) rng: Arc<dyn Rng>,
    /// The vault key that seals each Box's CA; `None` outside a controller.
    pub(crate) vault_key: Option<PathBuf>,
    /// The state directory, where the front Proxy listens.
    pub(crate) state_dir: Option<PathBuf>,
    /// The keepers of this process.
    pub(crate) keepers: &'static Keepers,
    /// Where each Box's credential hosts go.
    pub(crate) route: EgressRoute,
    /// The key Box tokens are signed with.
    pub(crate) key: Zeroizing<Vec<u8>>,
    /// The forwarder binary delivered into each Box.
    pub(crate) binary: PathBuf,
}

/// A store failure as a Proxy host failure.
fn failed(e: &StoreError) -> ProxyHostError {
    ProxyHostError::Failed(format!("the proxy state could not be used: {e}"))
}

impl KeptProxy {
    /// The Box's row, reserved first when it has none. A row is kept as it is, with its ports and
    /// its delivered token, so a controller restart never makes a Box's forwarder start again;
    /// only a row from before the egress forwarder is given a forward port.
    fn row(&self, box_id: BoxId) -> Result<ProxyRow, ProxyHostError> {
        let store = &self.deps.store;
        match store.proxy(box_id).map_err(|e| failed(&e))? {
            Some(row) if row.forward_port.is_some() => return Ok(row),
            Some(_) => store.give_forward_port(box_id, self.rng.as_ref()),
            None => store.reserve(box_id, self.rng.as_ref()).map(drop),
        }
        .map_err(|e| failed(&e))?;
        store
            .proxy(box_id)
            .map_err(|e| failed(&e))?
            .ok_or_else(|| ProxyHostError::Failed("the proxy row is gone".to_owned()))
    }

    /// The Box's CA certificate, made and sealed first when the row has none.
    fn ca_pem(&self, box_id: BoxId) -> Result<String, ProxyHostError> {
        if let Some(ca) = self.deps.store.proxy_ca(box_id).map_err(|e| failed(&e))? {
            return Ok(ca.cert_pem);
        }
        let key = self.vault_key.as_deref().ok_or_else(|| {
            ProxyHostError::Failed("only a controller can make a Box's CA".to_owned())
        })?;
        let vault = Vault::load(key).map_err(|e| ProxyHostError::Failed(e.to_string()))?;
        let pair = box_ca((&self.deps.store, &vault), box_id).map_err(|f| {
            ProxyHostError::Failed(match f {
                Fault::Refused(why) | Fault::Store(why) => why,
            })
        })?;
        Ok(pair.cert_pem.clone())
    }
}

impl ProxyHost for KeptProxy {
    fn front_ca_pem(&self) -> Result<String, String> {
        let state = self
            .state_dir
            .as_ref()
            .ok_or("a front Proxy needs a state dir")?;
        crate::proxy::fronts::ca_pair(state)
            .map(|pair| pair.cert_pem.clone())
            .map_err(|e| format!("cannot read the front Proxy CA: {e}"))
    }

    fn front_running(&self, front: &str) -> bool {
        self.state_dir
            .as_ref()
            .is_some_and(|dir| crate::proxy::fronts::running(dir, front))
    }

    /// Records `spec`, starts the Box's keeper when it has none, brings its tunnel up and
    /// delivers the Box's forwarder with a token unless it is in place.
    fn ensure(&self, spec: &ProxySpec) -> Result<ProxyEndpoint, ProxyHostError> {
        let box_id = spec.box_id;
        let row = self.row(box_id)?;
        self.deps
            .store
            .set_proxy_spec(box_id, spec)
            .map_err(|e| failed(&e))?;
        let ca_pem = self.ca_pem(box_id)?;
        let kept = self.keepers.start(&self.deps, box_id, row.box_port);
        kept.bring_up().map_err(ProxyHostError::Failed)?;
        let mut ep = ProxyEndpoint {
            box_port: kept.box_port(),
            egress_port: row.egress_port,
            forward_port: row.forward_port,
            ca_pem,
        };
        let delivery = Delivery {
            route: &self.route,
            key: &self.key,
            driver: self.deps.driver.as_ref(),
            store: &self.deps.store,
            box_id,
            handle: spec.box_handle.as_ref(),
            binary: &self.binary,
        };
        ep.forward_port = Some(deliver(&delivery, &ep).map_err(ProxyHostError::Failed)?);
        Ok(ep)
    }

    fn restart(&self, box_id: BoxId) -> Result<(), ProxyHostError> {
        let spec = self
            .deps
            .store
            .proxy_spec(box_id)
            .map_err(|e| failed(&e))?
            .ok_or_else(|| {
                ProxyHostError::Failed("no spec to restart the proxy from".to_owned())
            })?;
        self.keepers.end(box_id);
        self.ensure(&spec).map(drop)
    }

    fn status(&self, box_id: BoxId) -> Result<ProxyStatus, ProxyHostError> {
        let row = self.deps.store.proxy(box_id).map_err(|e| failed(&e))?;
        Ok(match (row, self.keepers.get(box_id)) {
            (None, _) => ProxyStatus::Absent,
            (Some(_), None) => ProxyStatus::Stale,
            (Some(_), Some(kept)) if kept.up() => ProxyStatus::Ready {
                box_port: kept.box_port(),
            },
            (Some(_), Some(_)) => ProxyStatus::Starting,
        })
    }

    /// Ends the Box's keeper, letting its machine go when the Box closed, and removes its row.
    fn stop(&self, box_id: BoxId) -> Result<(), ProxyHostError> {
        if let Some(kept) = self.keepers.end(box_id) {
            kept.close();
            return Ok(());
        }
        self.deps.store.remove_proxy(box_id).map(drop).map_err(|e| {
            ProxyHostError::Incomplete(format!("the proxy row could not be removed: {e}"))
        })
    }

    fn park(&self, box_id: BoxId) -> Result<(), ProxyHostError> {
        if let Some(kept) = self.keepers.get(box_id) {
            kept.park();
        }
        Ok(())
    }

    /// Opens the tunnel of the Box's keeper again, starting the keeper when the Box has a row
    /// and none runs.
    fn restore(&self, box_id: BoxId) -> Result<(), ProxyHostError> {
        let Some(kept) = self.keepers.get(box_id) else {
            let row = self.deps.store.proxy(box_id).map_err(|e| failed(&e))?;
            return row.map_or(Ok(()), |_| self.restart(box_id));
        };
        kept.restore().map_err(|why| {
            ProxyHostError::Failed(format!("the tunnel did not come back up: {why}"))
        })
    }
}

/// The Proxy host outside a controller, where no Box runs.
pub(crate) struct NoProxyHost;

/// Why [`NoProxyHost`] refuses.
fn no_controller() -> ProxyHostError {
    ProxyHostError::Failed("only a controller runs Box Proxies".to_owned())
}

impl ProxyHost for NoProxyHost {
    fn front_ca_pem(&self) -> Result<String, String> {
        Err("this host runs no Proxy".to_owned())
    }

    fn front_running(&self, _front: &str) -> bool {
        false
    }

    fn ensure(&self, _spec: &ProxySpec) -> Result<ProxyEndpoint, ProxyHostError> {
        Err(no_controller())
    }

    fn restart(&self, _box_id: BoxId) -> Result<(), ProxyHostError> {
        Err(no_controller())
    }

    fn status(&self, _box_id: BoxId) -> Result<ProxyStatus, ProxyHostError> {
        Ok(ProxyStatus::Absent)
    }

    fn stop(&self, _box_id: BoxId) -> Result<(), ProxyHostError> {
        Err(no_controller())
    }

    fn park(&self, _box_id: BoxId) -> Result<(), ProxyHostError> {
        Ok(())
    }

    fn restore(&self, _box_id: BoxId) -> Result<(), ProxyHostError> {
        Ok(())
    }
}
