//! `list_nodes`: the nodes the thread's starter may start a Box on, and where a start goes now.

use moor_core::request::{AsReq, Request};
use moor_core::response::ExitClass;
use moor_core::view::node::NodeListValue;

use super::{error_text, Reply, Scope};

/// `list_nodes`: asks the controller as the starter and renders its answer as lines of text.
pub(super) fn list(scope: &mut Scope<'_>) -> Reply {
    match (scope.call)(Request::NodeList(AsReq { identity: None })) {
        Ok(r) if r.class == ExitClass::Ok => r
            .value
            .and_then(|v| serde_json::from_value::<NodeListValue>(v).ok())
            .map_or_else(
                || Reply::refused("The controller's node list was not readable."),
                |listed| Reply::done(listed.text()),
            ),
        Ok(r) => Reply::refused(format!("The controller refused: {}", error_text(r))),
        Err(e) => Reply::refused(format!("The controller did not answer: {e}")),
    }
}
