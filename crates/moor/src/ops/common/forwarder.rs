//! Delivering the in-Box egress forwarder, its config and token into a Box.

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use moor_core::exec_env::FORWARDER;
use moor_core::seam::driver::Driver;
use moor_core::seam::proxy_host::ProxyEndpoint;
use moor_core::{BoxHandle, BoxId, BoxPath};

use crate::cx::egress::EgressRoute;
use crate::proxy::token;
use crate::store::Store;

/// Keeps two deliveries from both minting, so the token the row holds is the one in the Box.
static DELIVERING: Mutex<()> = Mutex::new(());

/// What a delivery reaches.
pub(crate) struct Delivery<'a> {
    /// The route the Box's credential hosts take.
    pub(crate) route: &'a EgressRoute,
    /// The key the shared Proxy checks token MACs with.
    pub(crate) key: &'a [u8],
    /// Writes into the Box.
    pub(crate) driver: &'a dyn Driver,
    /// Holds the Box's token.
    pub(crate) store: &'a Store,
    /// The Box.
    pub(crate) box_id: BoxId,
    /// The Box's handle; none before it has one.
    pub(crate) handle: Option<&'a BoxHandle>,
    /// The forwarder binary on this host.
    pub(crate) binary: &'a Path,
}

/// Returns the port tinyproxy sends credential hosts to. On the shared Proxy's routes that is the
/// forwarder's, and until a token reached the Box a fresh one is minted, the forwarder and then
/// its config with that token are written into the Box, and the token is marked delivered.
///
/// # Errors
/// The Box's Proxy has no forward port, or the token cannot be minted, or the forwarder cannot
/// be read or written.
pub(crate) fn deliver(to: &Delivery<'_>, ep: &ProxyEndpoint) -> Result<u16, String> {
    let Some(upstream) = to.route.upstream(ep.box_port) else {
        return Ok(ep.box_port);
    };
    let port = ep
        .forward_port
        .ok_or("this Box's Proxy predates the egress forwarder; start the Box again")?;
    let delivered = || {
        to.store
            .egress_token_delivered(to.box_id)
            .map_err(|e| e.to_string())
    };
    if delivered()? {
        return Ok(port);
    }
    let _one = DELIVERING.lock().unwrap_or_else(PoisonError::into_inner);
    if !delivered()? {
        write_into(to, &upstream, port)?;
    }
    Ok(port)
}

/// Mints the Box's token, writes the forwarder, then its config listening on `port`, into the
/// Box, and marks the token delivered.
fn write_into(to: &Delivery<'_>, upstream: &str, port: u16) -> Result<(), String> {
    let handle = to
        .handle
        .ok_or("the egress forwarder needs a Box to deliver to")?;
    let bin = std::fs::read(to.binary).map_err(|e| {
        format!(
            "cannot read the egress forwarder {}: {e}",
            to.binary.display()
        )
    })?;
    let home = box_home(to.driver, handle)?;
    let token = token::mint(to.store, to.key, to.box_id)?
        .ok_or("the Box has no Proxy to mint an egress token for")?;
    let config = serde_json::json!({
        "listen": format!("127.0.0.1:{port}"),
        "upstream": upstream,
        "token": String::from_utf8_lossy(token.expose()),
    });
    let put = |suffix: &str, bytes: &[u8], mode: u32| {
        let path = BoxPath::under(home.as_str(), &format!("{FORWARDER}{suffix}"))
            .map_err(|e| e.to_string())?;
        to.driver
            .put_file(handle, &path, bytes, mode)
            .map_err(|e| format!("cannot deliver the egress forwarder: {e}"))
    };
    put(".new", &bin, 0o755)?;
    put(".json", config.to_string().as_bytes(), 0o600)?;
    to.store
        .note_egress_token_delivered(to.box_id, token.expose())
        .map(drop)
        .map_err(|e| e.to_string())
}

/// The Box user's home, from the environment a command in the Box starts with.
fn box_home(driver: &dyn Driver, handle: &BoxHandle) -> Result<BoxPath, String> {
    let environ = BoxPath::restore("/proc/self/environ").map_err(|e| e.to_string())?;
    let text = driver
        .peek_file(handle, &environ)
        .map_err(|e| format!("cannot find the Box user's home: {e}"))?
        .unwrap_or_default();
    let home = text
        .split('\0')
        .find_map(|v| v.strip_prefix("HOME="))
        .ok_or("the Box user has no home directory")?;
    BoxPath::restore(home).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
