//! rsync's remote shell for a Box: `moor-machine-runner box-rsh <box> <host> <command…>` sends
//! the command to the controller as `box-rsync`, which runs it in the Box, and carries rsync's
//! stdio over that one connection. The runner hands the helper its dial in the environment.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use moor_client::link::ops;
use moor_client::Client;
use moor_core::request::{BoxRsyncReq, Request};
use moor_core::BoxId;

const SOCKET: &str = "MOOR_RUNNER_SOCKET";
const CONTROLLER: &str = "MOOR_RUNNER_CONTROLLER";
const CA: &str = "MOOR_RUNNER_CONTROLLER_CA";
const KEY: &str = "MOOR_MACHINE_SERVICE_KEY";

/// How the runner reaches the controller.
#[derive(Debug, Clone)]
pub enum Dial {
    /// The controller's socket for the runner.
    Socket(PathBuf),
    /// The controller's TLS listener.
    Tls {
        /// `tls://HOST:PORT`.
        url: String,
        /// The PEM CA that signed the listener's certificate.
        ca: PathBuf,
        /// The machine service key.
        key: String,
    },
}

impl Dial {
    /// A client for the controller this dial names.
    ///
    /// # Errors
    /// The reason a TLS dial cannot be set up, such as an unreadable CA.
    pub fn client(&self) -> Result<Client, String> {
        match self {
            Self::Socket(socket) => Ok(Client::new(socket)),
            Self::Tls { url, ca, key } => Client::runner(url, ca, key),
        }
    }

    /// The dial `--socket`, or `--controller` with its CA and key, names, and the path the
    /// runner's own sockets sit beside: `socket`, or one in `state_dir` for `--controller`.
    ///
    /// # Errors
    /// Names the flag missing.
    pub fn from_flags(
        socket: Option<&Path>,
        (url, ca, key): (&Option<String>, &Option<PathBuf>, &Option<String>),
        state_dir: &Path,
    ) -> Result<(Self, PathBuf), String> {
        let Some(url) = url.clone() else {
            let socket = socket.ok_or("give --socket or --controller")?.to_path_buf();
            return Ok((Self::Socket(socket.clone()), socket));
        };
        let ca = ca.clone().ok_or("--controller needs --controller-ca")?;
        let key = key.clone().filter(|k| !k.is_empty());
        let key = key.ok_or("--controller needs --service-key")?;
        Ok((Self::Tls { url, ca, key }, state_dir.join("runner.sock")))
    }

    /// The environment that hands this dial to a helper.
    fn env(&self) -> Vec<(&'static str, OsString)> {
        match self {
            Self::Socket(socket) => vec![(SOCKET, socket.into())],
            Self::Tls { url, ca, key } => {
                vec![(CONTROLLER, url.into()), (CA, ca.into()), (KEY, key.into())]
            }
        }
    }

    /// The dial the runner handed this helper.
    fn from_env() -> Result<Self, String> {
        let var = |name| std::env::var_os(name).filter(|v| !v.is_empty());
        if let Some(socket) = var(SOCKET) {
            return Ok(Self::Socket(socket.into()));
        }
        let text = |name| {
            var(name)
                .and_then(|v| v.into_string().ok())
                .ok_or_else(|| format!("{name} is not set"))
        };
        Ok(Self::Tls {
            url: text(CONTROLLER)?,
            ca: text(CA)?.into(),
            key: text(KEY)?,
        })
    }
}

/// `rsync` with the Box `box_id` reachable as host `box` through `dial`.
pub(crate) fn rsync(dial: &Dial, box_id: BoxId) -> Result<Command, String> {
    let exe =
        std::env::current_exe().map_err(|e| format!("cannot find the runner's own binary: {e}"))?;
    let helper = ops::quote(&exe.display().to_string());
    let mut rsync = Command::new("rsync");
    rsync
        .arg("-e")
        .arg(format!("{helper} box-rsh {}", box_id.get()))
        .envs(dial.env());
    Ok(rsync)
}

/// The helper: `args` are `<box> <host> <command…>` as rsync runs its remote shell. Its exit
/// code, for the process to exit with at once, since stdin's reader may still be blocked.
#[must_use]
pub fn box_rsh(args: &[String]) -> i32 {
    match bridge(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("moor-machine-runner box-rsh: {e}");
            1
        }
    }
}

fn bridge(args: &[String]) -> Result<i32, String> {
    let [box_id, _host, command @ ..] = args else {
        return Err("usage: moor-machine-runner box-rsh <box> <host> <command>…".to_owned());
    };
    let id = box_id
        .parse()
        .map(BoxId::new)
        .map_err(|_| format!("{box_id:?} is not a Box id"))?;
    let client = Dial::from_env()?.client()?;
    let request = Request::BoxRsync(BoxRsyncReq {
        id,
        argv: command.to_vec(),
    });
    let response = client
        .pipe(
            request,
            std::io::stdin(),
            &mut std::io::stdout().lock(),
            &mut std::io::stderr(),
        )
        .map_err(|e| format!("the controller did not run rsync: {e}"))?;
    if let Some(error) = response.error {
        return Err(error.message);
    }
    let exit = response.value.as_ref().and_then(|v| v.get("exit"));
    let code = exit.and_then(serde_json::Value::as_i64).unwrap_or(1);
    Ok(i32::try_from(code).unwrap_or(1))
}
