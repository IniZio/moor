//! The `moor-gate` binary.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use moor_gate::{Gate, Mcp, Verifier};

/// The remote front: OIDC access tokens in, the controller's front socket out.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Address to listen on; TLS is the ingress's job.
    #[arg(long, env = "MOOR_GATE_LISTEN", default_value = "0.0.0.0:8443")]
    listen: String,
    /// The controller's front socket for the `authgear` front.
    #[arg(long, env = "MOOR_GATE_FRONT_SOCKET")]
    front_socket: PathBuf,
    /// The OIDC issuer tokens must name in `iss`.
    #[arg(long, env = "MOOR_GATE_ISSUER")]
    issuer: String,
    /// The client id tokens must name in `aud`.
    #[arg(long, env = "MOOR_GATE_CLIENT_ID")]
    client_id: String,
    /// Where the issuer's keys are, when the gate cannot reach the issuer's discovery URL.
    #[arg(long, env = "MOOR_GATE_JWKS_URL")]
    jwks_url: Option<String>,
    /// The public URL of `/mcp`, such as `https://moor.example.com/mcp`; tokens for bots must
    /// name it in `aud`. `/mcp` is off without it.
    #[arg(long, env = "MOOR_GATE_MCP_RESOURCE_URL")]
    mcp_resource_url: Option<String>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let listener = match TcpListener::bind(&args.listen) {
        Ok(listener) => listener,
        Err(e) => {
            moor_gate_log(&format!("cannot listen on {}: {e}", args.listen));
            return ExitCode::FAILURE;
        }
    };
    let mcp = match args
        .mcp_resource_url
        .map(|url| Mcp::new(&url, args.front_socket.clone()))
        .transpose()
    {
        Ok(mcp) => mcp,
        Err(why) => {
            moor_gate_log(&why);
            return ExitCode::FAILURE;
        }
    };
    let gate = Arc::new(Gate {
        front_socket: args.front_socket,
        mcp,
        verifier: Verifier::new(args.issuer, args.client_id, args.jwks_url),
    });
    match moor_gate::serve(&listener, &gate) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            moor_gate_log(&format!("accept failed: {e}"));
            ExitCode::FAILURE
        }
    }
}

/// Writes one `moor-gate: <msg>` line to stderr.
fn moor_gate_log(msg: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "moor-gate: {msg}");
}
