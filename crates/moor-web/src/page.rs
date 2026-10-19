//! The single static page and its script, served from the binary.

use crate::http::{Handler, HttpRequest, HttpResponse, Reply};

#[cfg(test)]
mod tests;

/// The page markup.
const INDEX_HTML: &str = include_str!("page/index.html");
/// The page script.
const APP_JS: &str = include_str!("page/app.js");

/// Serves `/` and `/app.js`; every other request goes to `fallback`.
#[must_use]
pub fn handler(fallback: Handler) -> Handler {
    std::sync::Arc::new(move |req: &HttpRequest| {
        let mut resp = match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/") => HttpResponse::html(INDEX_HTML),
            ("GET", "/app.js") => {
                let mut resp = HttpResponse::html(APP_JS);
                resp.content_type = "text/javascript; charset=utf-8";
                resp
            }
            _ => return fallback(req),
        };
        resp.headers.push(("Cache-Control", "no-store".to_owned()));
        Reply::Full(resp)
    })
}
