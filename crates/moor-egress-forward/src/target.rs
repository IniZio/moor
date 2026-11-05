//! The target of a plain `http://` request.

use tokio_tungstenite::tungstenite::http::StatusCode;

/// The port of an `http://` target that names none.
const HTTP_PORT: u16 = 80;

/// The `host:port` of an absolute `http://` request target, port 80 when it names none.
pub(super) fn plain_target(path: &str) -> Result<String, StatusCode> {
    let rest = path
        .get(..7)
        .filter(|s| s.eq_ignore_ascii_case("http://"))
        .and_then(|_| path.get(7..))
        .ok_or(StatusCode::METHOD_NOT_ALLOWED)?;
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    let named = authority
        .rsplit_once(':')
        .filter(|(_, p)| !p.contains(']'))
        .map(|(h, p)| (h, p.parse::<u16>().ok()));
    let (host, port) = named.unwrap_or((authority, Some(HTTP_PORT)));
    match (host.is_empty(), port) {
        (false, Some(port)) => Ok(format!("{host}:{port}")),
        _ => Err(StatusCode::BAD_REQUEST),
    }
}
