//! The `moor-gate` binary.

use std::io::Read as _;
use std::net::{TcpListener, ToSocketAddrs as _};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use moor_client::tls::TlsPki;
use moor_client::Client;
use moor_gate::{Boxes, Gate, Login, Mcp, Verifier, DEFAULT_BOX_LISTEN, SERVICE_KEY_ENV};

/// The remote front: OIDC access tokens in, the controller out.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Address to listen on; TLS is the ingress's job. `off` serves only the Box-facing
    /// listener, with no sign-in, `/mcp` or account page, and then `--issuer` and `--client-id`
    /// are not needed.
    #[arg(long, env = "MOOR_GATE_LISTEN", default_value = "0.0.0.0:8443")]
    listen: String,
    /// Address of the Box-facing listener, which serves only `report_step` to the agent in a
    /// Box and trusts the `X-Moor-Box` header the Proxy stamps. Bind loopback beside a Proxy on
    /// the host, or the pod address on a cluster, where the gate's Service reaches the Proxy.
    /// Nothing but a Proxy may reach it. `off` turns it off. A non-loopback address is refused
    /// unless `--box-listen-off-host` is also set.
    #[arg(long, env = "MOOR_GATE_BOX_LISTEN", default_value = DEFAULT_BOX_LISTEN)]
    box_listen: String,
    /// Allows `--box-listen` to bind an address that is not loopback, such as the pod address
    /// on a cluster. Set it only where nothing but a Proxy can reach that address, because the
    /// listener trusts the `X-Moor-Box` header it is sent. Where a `NetworkPolicy` cannot keep
    /// other callers off it, set `MOOR_SERVICE_KEY` instead: the listener then refuses, with 401,
    /// any request whose `X-Moor-Service-Key` header, which the Proxy stamps, differs. Set but
    /// empty, the gate does not start.
    #[arg(long, env = "MOOR_GATE_BOX_LISTEN_OFF_HOST")]
    box_listen_off_host: bool,
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
    /// The OIDC issuer tokens must name in `iss`; needed unless `--listen` is `off`.
    #[arg(long, env = "MOOR_GATE_ISSUER")]
    issuer: Option<String>,
    /// The client id tokens must name in `aud`; needed unless `--listen` is `off`.
    #[arg(long, env = "MOOR_GATE_CLIENT_ID")]
    client_id: Option<String>,
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
    /// the gate keeps a key of its own in `$XDG_STATE_HOME/moor-gate/cookie-key`, so sign-in
    /// works on one replica only but outlives a restart.
    #[arg(long, env = "MOOR_GATE_COOKIE_KEY", hide_env_values = true)]
    cookie_key: Option<String>,
    /// Seconds a SIGTERM waits for connections in flight before the gate exits.
    #[arg(long, env = "MOOR_GATE_DRAIN_TIMEOUT_SECS", default_value_t = 30)]
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

/// The public listener, bound; none when `--listen` is `off`.
fn public_listener(args: &Args) -> Result<Option<TcpListener>, String> {
    if args.listen == "off" {
        return Ok(None);
    }
    if args.issuer.is_none() || args.client_id.is_none() {
        return Err("--issuer and --client-id are needed unless --listen is off".to_owned());
    }
    TcpListener::bind(&args.listen)
        .map(Some)
        .map_err(|e| format!("cannot listen on {}: {e}", args.listen))
}

fn main() -> ExitCode {
    let args = Args::parse();
    let controller = match args.client() {
        Ok(controller) => controller,
        Err(why) => return fail(&why),
    };
    let public = match public_listener(&args) {
        Ok(public) => public,
        Err(why) => return fail(&why),
    };
    let boxes = service_key(std::env::var(SERVICE_KEY_ENV).ok())
        .and_then(|key| {
            guarded_box_listen(&args.box_listen, args.box_listen_off_host).map(|()| key)
        })
        .and_then(|key| serve_boxes(&args.box_listen, &controller, key));
    if let Err(why) = boxes {
        return fail(&why);
    }
    let wake = match moor_gate::on_termination() {
        Ok(wake) => wake,
        Err(e) => {
            moor_gate_log(&format!("cannot handle SIGTERM: {e}"));
            return ExitCode::FAILURE;
        }
    };
    if let (Some(listener), Some(issuer), Some(client_id)) =
        (public, args.issuer.clone(), args.client_id.clone())
    {
        serve_public(args, controller, listener, (issuer, client_id), &wake)
    } else {
        let _ = (&wake).read(&mut [0_u8; 1]);
        ExitCode::SUCCESS
    }
}

/// Serves the public listener until `wake` turns readable, then drains.
fn serve_public(
    args: Args,
    controller: Client,
    listener: TcpListener,
    (issuer, client_id): (String, String),
    wake: &UnixStream,
) -> ExitCode {
    let mcp = match args
        .mcp_resource_url
        .map(|url| Mcp::with_setup(&url, controller.clone(), args.public_url.as_deref()))
        .transpose()
    {
        Ok(mcp) => mcp,
        Err(why) => return fail(&why),
    };
    let sign_in = |url: &str| {
        args.cookie_key
            .as_deref()
            .map_or_else(|| kept_key_login(url), |keys| Login::with_keys(url, keys))
    };
    let login = match args.public_url.as_deref().map(sign_in).transpose() {
        Ok(login) => login.map(Arc::new),
        Err(why) => return fail(&why),
    };
    let gate = Arc::new(Gate {
        controller,
        mcp,
        verifier: Verifier::new(issuer, client_id, args.jwks_url),
    });
    let drain = Duration::from_secs(args.drain_timeout_secs);
    match moor_gate::serve_until(listener, &gate, login.as_ref(), wake, drain) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            moor_gate_log(&format!("accept failed: {e}"));
            ExitCode::FAILURE
        }
    }
}

/// Sign-in under the key kept in the state folder. A gate that cannot keep one seals under a key
/// of its own process, so every restart ends every session; it says so.
fn kept_key_login(url: &str) -> Result<Login, String> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")));
    let kept = state
        .ok_or_else(|| "neither XDG_STATE_HOME nor HOME is set".to_owned())
        .and_then(|dir| Login::with_kept_key(url, &dir.join("moor-gate/cookie-key")));
    kept.or_else(|why| {
        moor_gate_log(&format!(
            "cannot keep a cookie key ({why}); sign-ins end when this gate restarts. Set --cookie-key."
        ));
        Login::new(url)
    })
}

/// Refuses a `--box-listen` that is not loopback unless `off_host` opts in, so no caller off the
/// host can name a Box unchecked. `off` and an address that does not resolve pass: the first
/// binds nothing and the second fails on bind.
fn guarded_box_listen(listen: &str, off_host: bool) -> Result<(), String> {
    if listen == "off" || off_host {
        return Ok(());
    }
    let reaches_off_host = listen
        .to_socket_addrs()
        .is_ok_and(|mut addrs| addrs.any(|a| !a.ip().is_loopback()));
    if reaches_off_host {
        return Err(format!(
            "--box-listen {listen} is not loopback, so it needs --box-listen-off-host"
        ));
    }
    Ok(())
}

/// The shared secret `MOOR_SERVICE_KEY` names, if it is set; a set but empty value is an error,
/// so a gate never runs open by a typo.
fn service_key(raw: Option<String>) -> Result<Option<String>, String> {
    match raw {
        Some(key) if key.is_empty() => Err(format!(
            "{SERVICE_KEY_ENV} is set but empty; set it to the shared secret or unset it"
        )),
        key => Ok(key),
    }
}

/// Starts the Box-facing listener on `listen` in a thread of its own, unless it is `off`. With a
/// `key`, the listener refuses requests that do not carry it.
fn serve_boxes(listen: &str, controller: &Client, key: Option<String>) -> Result<(), String> {
    if listen == "off" {
        return Ok(());
    }
    let listener = TcpListener::bind(listen)
        .map_err(|e| format!("cannot listen on {listen} for Boxes: {e}"))?;
    let boxes = Boxes::new(controller.clone())?;
    let boxes = match key {
        Some(key) => boxes.requiring_key(key),
        None => boxes,
    };
    std::thread::spawn(move || {
        if let Err(e) = boxes.serve(listener) {
            moor_gate_log(&format!("the Box listener stopped: {e}"));
        }
    });
    Ok(())
}

/// Logs `why` and gives the exit code of a gate that cannot run.
fn fail(why: &str) -> ExitCode {
    moor_gate_log(why);
    ExitCode::FAILURE
}

/// Writes one `moor-gate: <msg>` line to stderr.
fn moor_gate_log(msg: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "moor-gate: {msg}");
}

#[cfg(test)]
mod tests {
    use super::{guarded_box_listen, service_key};

    #[test]
    fn the_service_key_is_unset_or_non_empty() {
        assert_eq!(service_key(None), Ok(None));
        assert_eq!(service_key(Some("k".to_owned())), Ok(Some("k".to_owned())));
        let why = service_key(Some(String::new())).unwrap_err();
        assert!(why.contains("MOOR_SERVICE_KEY"), "{why}");
    }

    #[test]
    fn a_box_listener_off_the_loopback_needs_the_opt_in() {
        for addr in ["127.0.0.1:8444", "[::1]:8444", "localhost:8444", "off"] {
            assert_eq!(guarded_box_listen(addr, false), Ok(()), "{addr}");
        }
        for addr in ["0.0.0.0:8444", "10.1.2.3:8444", "[::]:8444"] {
            let why = guarded_box_listen(addr, false).unwrap_err();
            assert!(why.contains("--box-listen-off-host"), "{addr}: {why}");
            assert_eq!(guarded_box_listen(addr, true), Ok(()), "{addr}");
        }
    }
}
