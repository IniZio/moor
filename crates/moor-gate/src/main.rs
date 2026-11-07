//! The `moor-gate` binary.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use moor_client::tls::TlsPki;
use moor_client::Client;
use moor_gate::{Gate, Login, Mcp, Verifier};

/// The remote front: OIDC access tokens in, the controller out.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Address to listen on; TLS is the ingress's job.
    #[arg(long, env = "MOOR_GATE_LISTEN", default_value = "0.0.0.0:8443")]
    listen: String,
    /// The controller's front socket for the `authgear` front.
    #[arg(
        long,
        env = "MOOR_GATE_FRONT_SOCKET",
        required_unless_present = "controller",
        conflicts_with = "controller"
    )]
    front_socket: Option<PathBuf>,
    /// The controller's TLS listener, `tls://HOST:PORT`, in place of `--front-socket`. A request
    /// waits up to 30 s for it to answer, then gets 503 with `Retry-After`.
    #[arg(
        long,
        env = "MOOR_GATE_CONTROLLER",
        requires_all = ["controller_ca", "controller_cert", "controller_key", "front_key"]
    )]
    controller: Option<String>,
    /// The CA that signed the controller's certificate.
    #[arg(long, env = "MOOR_GATE_CONTROLLER_CA")]
    controller_ca: Option<PathBuf>,
    /// The certificate presented to `--controller`.
    #[arg(long, env = "MOOR_GATE_CONTROLLER_CERT")]
    controller_cert: Option<PathBuf>,
    /// The private key of `--controller-cert`.
    #[arg(long, env = "MOOR_GATE_CONTROLLER_KEY")]
    controller_key: Option<PathBuf>,
    /// The key the front hello presents to `--controller`.
    #[arg(long, env = "MOOR_GATE_FRONT_KEY", hide_env_values = true)]
    front_key: Option<String>,
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
    /// The gate's public origin, such as `https://moor.example.com`; browser sign-in at
    /// `/auth/*` is off without it, and the issuer must allow `<public-url>/auth/callback`.
    #[arg(long, env = "MOOR_GATE_PUBLIC_URL")]
    public_url: Option<String>,
    /// Secrets of 32 or more characters, comma-separated, that seal the sign-in cookies. Every
    /// gate replica needs the same value; the first seals and all open, to rotate. Without it
    /// each gate seals under a key of its own, so sign-in works on one replica only.
    #[arg(long, env = "MOOR_GATE_COOKIE_KEY", hide_env_values = true)]
    cookie_key: Option<String>,
    /// Seconds a SIGTERM waits for connections in flight before the gate exits.
    #[arg(long, env = "MOOR_GATE_DRAIN_TIMEOUT_SECS", default_value_t = 600)]
    drain_timeout_secs: u64,
}

impl Args {
    /// The client these flags name.
    fn client(&self) -> Result<Client, String> {
        if let (Some(url), Some(ca), Some(cert), Some(key), Some(front_key)) = (
            &self.controller,
            &self.controller_ca,
            &self.controller_cert,
            &self.controller_key,
            &self.front_key,
        ) {
            let pki = TlsPki {
                ca: ca.clone(),
                cert: cert.clone(),
                key: key.clone(),
            };
            return moor_gate::over_tls(url, &pki, front_key, moor_gate::HOLD);
        }
        self.front_socket
            .as_ref()
            .map(Client::new)
            .ok_or_else(|| "set --front-socket or --controller".to_owned())
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    let controller = match args.client() {
        Ok(controller) => controller,
        Err(why) => {
            moor_gate_log(&why);
            return ExitCode::FAILURE;
        }
    };
    let listener = match TcpListener::bind(&args.listen) {
        Ok(listener) => listener,
        Err(e) => {
            moor_gate_log(&format!("cannot listen on {}: {e}", args.listen));
            return ExitCode::FAILURE;
        }
    };
    let mcp = match args
        .mcp_resource_url
        .map(|url| Mcp::with_setup(&url, controller.clone(), args.public_url.as_deref()))
        .transpose()
    {
        Ok(mcp) => mcp,
        Err(why) => {
            moor_gate_log(&why);
            return ExitCode::FAILURE;
        }
    };
    let sign_in = |url: &str| {
        args.cookie_key
            .as_deref()
            .map_or_else(|| Login::new(url), |keys| Login::with_keys(url, keys))
    };
    let login = match args.public_url.as_deref().map(sign_in).transpose() {
        Ok(login) => login.map(Arc::new),
        Err(why) => {
            moor_gate_log(&why);
            return ExitCode::FAILURE;
        }
    };
    let gate = Arc::new(Gate {
        controller,
        mcp,
        verifier: Verifier::new(args.issuer, args.client_id, args.jwks_url),
    });
    let wake = match moor_gate::on_termination() {
        Ok(wake) => wake,
        Err(e) => {
            moor_gate_log(&format!("cannot handle SIGTERM: {e}"));
            return ExitCode::FAILURE;
        }
    };
    let drain = Duration::from_secs(args.drain_timeout_secs);
    match moor_gate::serve_until(listener, &gate, login.as_ref(), &wake, drain) {
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
