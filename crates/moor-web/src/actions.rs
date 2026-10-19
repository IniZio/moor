//! Browser actions: steer, answer, end a Session, close a Box.
//!
//! Each action makes exactly one socket request and returns the controller's envelope
//! unchanged. Auth and CSRF are enforced by the HTTP server before the handler runs.

use crate::client::Client;
use crate::http::{Handler, HttpRequest, HttpResponse, Reply};

pub mod start;
mod steer;

#[cfg(test)]
mod route_tests;
#[cfg(test)]
mod tests;

/// Serves the action paths on POST and hands everything else to `fallback`.
pub fn handler(client: Client, fallback: Handler) -> Handler {
    std::sync::Arc::new(move |req: &HttpRequest| -> Reply {
        if req.method == "POST" && steer::PATHS.contains(&req.path.as_str()) {
            run(&client, req).into()
        } else {
            fallback(req)
        }
    })
}

/// Builds the request, makes the one socket call and shapes the reply.
fn run(client: &Client, req: &HttpRequest) -> HttpResponse {
    let call = match steer::build(&req.path, &req.body) {
        Ok(call) => call,
        Err(msg) => return HttpResponse::text("400 Bad Request", msg),
    };
    match client.request(call, |_| {}) {
        Ok(envelope) => match serde_json::to_string(&envelope) {
            Ok(body) => HttpResponse {
                status: "200 OK",
                content_type: "application/json",
                headers: Vec::new(),
                body,
            },
            Err(e) => HttpResponse::text("500 Internal Server Error", e.to_string()),
        },
        Err(e) => HttpResponse::text("502 Bad Gateway", e.to_string()),
    }
}
