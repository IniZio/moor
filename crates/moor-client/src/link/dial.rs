//! Dialing a server's machine link.

use std::sync::Arc;

use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

use super::{url, DIAL_TIMEOUT};

/// A socket dialed to `server`'s link, verified against the bundled web roots.
pub type Dialed = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Dials `server`'s link.
///
/// # Errors
/// The address is bad, or the connection or upgrade fails or takes over [`DIAL_TIMEOUT`].
pub async fn dial(server: &str) -> Result<Dialed, String> {
    dial_found(server).await?.ok_or_else(|| {
        format!(
            "cannot reach {}: not found",
            url(server).unwrap_or_default()
        )
    })
}

/// As [`dial`], but `None` when the server answers the upgrade with 404: it does not route links.
///
/// # Errors
/// As [`dial`], for every other failure.
pub async fn dial_found(server: &str) -> Result<Option<Dialed>, String> {
    let url = url(server)?;
    let tls = Connector::Rustls(Arc::new(crate::remote::tls_config()?));
    let dialing = tokio_tungstenite::connect_async_tls_with_config(&url, None, false, Some(tls));
    let ws = tokio::time::timeout(DIAL_TIMEOUT, dialing)
        .await
        .map_err(|_| {
            format!(
                "cannot reach {url}: no answer in {}s",
                DIAL_TIMEOUT.as_secs()
            )
        })?;
    let (ws, _) = match ws {
        Ok(dialed) => dialed,
        Err(tokio_tungstenite::tungstenite::Error::Http(res))
            if res.status() == StatusCode::NOT_FOUND =>
        {
            return Ok(None);
        }
        Err(e) => return Err(format!("cannot reach {url}: {e}")),
    };
    Ok(Some(ws))
}
