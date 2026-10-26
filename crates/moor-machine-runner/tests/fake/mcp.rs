//! A stdio MCP server standing in for one of the machine's own, for the relay tests.
//!
//! Each start appends `<pid> <MOOR_BOX>` to `~/fake-starts`; each `tap`, `connect` and `stop` appends its arguments
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

/// Arguments of `connect`, echoed back.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct Connect {
    /// The port to connect on.
    port: i64,
    /// The device to connect to.
    device_id: String,
}

/// Arguments of `stop`, echoed back.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct Stop {
    /// The devices to stop.
    devices: Vec<String>,
}

/// What `update` returns.
#[derive(Debug, Serialize, schemars::JsonSchema)]
struct Updated {
    /// Always true.
    updated: bool,
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

    #[tool(description = "Echo the connection.")]
    fn connect(&self, Parameters(connect): Parameters<Connect>) -> Json<Connect> {
        note(
            "fake-calls",
            &serde_json::to_string(&connect).unwrap_or_default(),
        );
        Json(connect)
    }

    #[tool(description = "Echo the devices to stop.")]
    fn stop(&self, Parameters(stop): Parameters<Stop>) -> Json<Stop> {
        note(
            "fake-calls",
            &serde_json::to_string(&stop).unwrap_or_default(),
        );
        Json(stop)
    }

    #[tool(description = "Change the install every Box shares.")]
    fn update(&self) -> Json<Updated> {
        note("fake-calls", "update");
        Json(Updated { updated: true })
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
