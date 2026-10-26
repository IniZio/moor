//! A stdio MCP server standing in for one of the machine's own, for the relay tests.
//!
//! Each start appends `<pid> <MOOR_BOX>` to `~/fake-starts`; each `tap` appends its arguments
//! as JSON to `~/fake-calls` and echoes them back.
#![allow(
    clippy::unused_async_trait_impl,
    clippy::unused_self,
    reason = "the tool macros take &self and async handlers whether or not they use them"
)]

use std::io::Write;

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use serde::{Deserialize, Serialize};

/// Arguments of `tap`, echoed back.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct Tap {
    /// The device to tap on.
    udid: String,
    /// Where to tap.
    x: i64,
}

/// What `list-devices` lists.
#[derive(Debug, Serialize, schemars::JsonSchema)]
struct Devices {
    /// Device ids.
    devices: Vec<String>,
}

#[derive(Clone)]
struct Fake;

#[tool_router]
impl Fake {
    #[tool(description = "Echo the tap.")]
    fn tap(&self, Parameters(tap): Parameters<Tap>) -> Json<Tap> {
        note(
            "fake-calls",
            &serde_json::to_string(&tap).unwrap_or_default(),
        );
        Json(tap)
    }

    #[tool(name = "list-devices", description = "List the devices.")]
    fn list_devices(&self) -> Json<Devices> {
        Json(Devices {
            devices: vec!["a".to_owned(), "b".to_owned()],
        })
    }
}

#[tool_handler]
impl ServerHandler for Fake {}

/// Appends `line` to `~/<file>`.
fn note(file: &str, line: &str) {
    let path = std::path::Path::new(&std::env::var("HOME").unwrap_or_default()).join(file);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

#[tokio::main]
async fn main() {
    let id = std::env::var("MOOR_BOX").unwrap_or_default();
    note("fake-starts", &format!("{} {id}", std::process::id()));
    if let Ok(service) = Fake.serve(rmcp::transport::stdio()).await {
        let _ = service.waiting().await;
    }
}
