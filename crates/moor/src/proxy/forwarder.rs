//! Delivering the in-Box egress forwarder, its config and token into a Box.

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use moor_core::exec_env::FORWARDER;
use moor_core::seam::driver::Driver;
use moor_core::seam::proxy_host::ProxyEndpoint;
use moor_core::{BoxHandle, BoxId, BoxPath};

use crate::proxy::route::EgressRoute;
use crate::proxy::token;
use crate::store::Store;

/// Keeps two deliveries in this controller from minting over each other. Another controller
/// can still mint between this one's write and mark, which the mark catches.
static DELIVERING: Mutex<()> = Mutex::new(());

/// Tries of mint, write and mark before a delivery gives up.
pub(crate) const MINT_TRIES: u32 = 3;

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

/// Returns the port tinyproxy sends credential hosts to: the forwarder's. Unless the row marks a token delivered, a fresh token is minted, the forwarder and then its config with that token are written into
/// the Box, and the token is marked delivered.
///
/// # Errors
/// The Box's Proxy has no forward port, or the token cannot be minted or marked, or the
/// forwarder cannot be read or written.
pub(crate) fn deliver(to: &Delivery<'_>, ep: &ProxyEndpoint) -> Result<u16, String> {
    let upstream = to.route.upstream(ep.box_port);
    if to.key.is_empty() {
        return Err("MOOR_EGRESS_KEY is not set, so no egress token can be minted".to_owned());
    }
    let port = ep
        .forward_port
        .ok_or("this Box's Proxy predates the egress forwarder; start the Box again")?;
    let handle = to
        .handle
        .ok_or("the egress forwarder needs a Box to deliver to")?;
    if in_place(to)? {
        return Ok(port);
    }
    let _one = DELIVERING.lock().unwrap_or_else(PoisonError::into_inner);
    if in_place(to)? {
        return Ok(port);
    }
    let bin = std::fs::read(to.binary).map_err(|e| {
        format!(
            "cannot read the egress forwarder {}: {e}",
            to.binary.display()
        )
    })?;
    let home = box_home(to.driver, handle)?;
    let put = |suffix: &str, bytes: &[u8], mode: u32| {
        let path = BoxPath::under(home.as_str(), &format!("{FORWARDER}{suffix}"))
            .map_err(|e| e.to_string())?;
        to.driver
            .put_file(handle, &path, bytes, mode)
            .map_err(|e| format!("cannot deliver the egress forwarder: {e}"))
    };
    mint_until_marked(to.store, to.key, to.box_id, |token| {
        let config = serde_json::json!({
            "listen": format!("127.0.0.1:{port}"),
            "upstream": upstream,
            "token": token,
        });
        put(".new", &bin, 0o755)?;
        put(".json", config.to_string().as_bytes(), 0o600)
    })?;
    Ok(port)
}

/// Whether the row marks the Box's token delivered. moor clears the mark whenever it gives the
/// Box a fresh disk, so a delivered token is one the Box still holds.
fn in_place(to: &Delivery<'_>) -> Result<bool, String> {
    to.store
        .egress_token_delivered(to.box_id)
        .map_err(|e| e.to_string())
}

/// Mints the Box's token, hands it to `write`, then marks it delivered. When another controller
/// minted in between, the mark fails and this tries again, up to [`MINT_TRIES`] times.
///
/// # Errors
/// The token cannot be minted or marked, `write` fails, or every try lost its mark.
pub(crate) fn mint_until_marked(
    store: &Store,
    key: &[u8],
    box_id: BoxId,
    write: impl Fn(&str) -> Result<(), String>,
) -> Result<(), String> {
    for _ in 0..MINT_TRIES {
        let token = token::mint(store, key, box_id)?
            .ok_or("the Box has no Proxy to mint an egress token for")?;
        write(&String::from_utf8_lossy(token.expose()))?;
        let marked = store
            .note_egress_token_delivered(box_id, token.expose())
            .map_err(|e| e.to_string())?;
        if marked {
            return Ok(());
        }
    }
    Err(format!(
        "another controller replaced the egress token of Box {} {MINT_TRIES} times while this one delivered it; run the command again",
        box_id.get()
    ))
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
