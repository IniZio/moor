//! Dialling the controller: where it is, the backoff between attempts and the watch reader's
//! loop.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

use moor_client::tls::TlsPki;
use moor_client::{Client, ClientError, Watch, WatchItem};
use moor_core::controller::WatchFilter;
use moor_core::user::Identity;

/// The namespace this front's hello names.
const NAMESPACE: &str = "slack";

/// Where the front reaches the controller.
#[derive(Debug, Clone)]
pub enum Dial {
    /// The front socket beside the controller.
    Socket(PathBuf),
    /// The controller's TLS listener, from another pod.
    Tls {
        /// `tls://HOST:PORT`.
        url: String,
        /// The CA it trusts and the certificate it presents.
        pki: TlsPki,
        /// The file holding the key the hello presents, read on each [`Dial::client`].
        key_file: PathBuf,
    },
}

impl Dial {
    /// A client that speaks as this front.
    ///
    /// # Errors
    /// The key file or a PEM file cannot be read, or the address is not `tls://HOST:PORT`.
    pub fn client(&self) -> Result<Client, String> {
        match self {
            Self::Socket(path) => Ok(Client::new(path)),
            Self::Tls { url, pki, key_file } => {
                let key = crate::secret_file::read_secret(key_file, "--front-key-file")?;
                Client::front(url, pki, NAMESPACE, &key)
            }
        }
    }

    /// The tool server's flags for this dial, which name no secret. `socket` is the path of
    /// [`Dial::Socket`] as a process other than this one must dial it.
    pub(crate) fn tool_args(&self, socket: impl Fn(&Path) -> PathBuf) -> Vec<String> {
        let shown = |path: &Path| path.display().to_string();
        match self {
            Self::Socket(path) => vec!["--socket".to_owned(), shown(&socket(path))],
            Self::Tls { url, pki, key_file } => vec![
                "--controller".to_owned(),
                url.clone(),
                "--controller-ca".to_owned(),
                shown(&pki.ca),
                "--controller-cert".to_owned(),
                shown(&pki.cert),
                "--controller-key".to_owned(),
                shown(&pki.key),
                "--front-key-file".to_owned(),
                shown(key_file),
            ],
        }
    }
}

/// `path` as the tool server must dial it: absolute, with a leading `/proc/self` named as this
/// process's.
pub(crate) fn tool_socket(path: &Path) -> PathBuf {
    own_proc(&std::path::absolute(path).unwrap_or_else(|_| path.to_owned()))
}

/// `path` with a leading `/proc/self` named as this process's `/proc/<pid>`. A front's socket
/// is `/proc/self/fd/3`, a descriptor only this process holds; the agent does not hand it on,
/// so the tool server it starts must dial this process's descriptor by pid.
fn own_proc(path: &Path) -> PathBuf {
    path.strip_prefix("/proc/self").map_or_else(
        |_| path.to_owned(),
        |rest| {
            Path::new("/proc")
                .join(std::process::id().to_string())
                .join(rest)
        },
    )
}

/// First wait before dialling the controller again.
const BACKOFF_MIN: Duration = Duration::from_millis(250);

/// Longest wait between dials.
const BACKOFF_MAX: Duration = Duration::from_secs(10);

/// A wait between dials that doubles up to [`BACKOFF_MAX`].
#[derive(Debug)]
pub(crate) struct Backoff(Duration);

impl Backoff {
    /// Starts at [`BACKOFF_MIN`].
    pub(crate) const fn new() -> Self {
        Self(BACKOFF_MIN)
    }

    /// The wait to take now; the following one doubles.
    pub(crate) fn next(&mut self) -> Duration {
        let wait = self.0;
        self.0 = wait.saturating_mul(2).min(BACKOFF_MAX);
        wait
    }
}

/// What a watch reader forwards to the feed, with the identity whose watch it follows.
pub(crate) type Tagged = (Identity, Watched);

/// What the watch reader forwards to the feed.
#[derive(Debug)]
pub(crate) enum Watched {
    /// A new connection starts its backlog from the first event, so the feed folds afresh.
    Connected,
    /// One item of the stream.
    Item(WatchItem),
}

/// Why one watch connection ended early.
enum Ended {
    /// The controller could not be reached or broke the stream.
    Failed(ClientError),
    /// Nobody reads the forwarded items any more.
    FeedGone,
}

/// The watch reader's dial state: the next wait and the last failure it logged.
struct Dialer {
    /// Wait before the next dial.
    backoff: Backoff,
    /// Text of the last failure logged since the last healthy connection.
    logged: Option<String>,
}

impl Dialer {
    /// A connection delivered its backlog: dial again quickly and log the next failure.
    fn healthy(&mut self) {
        self.backoff = Backoff::new();
        self.logged = None;
    }

    /// Logs `e` unless it repeats the last failure logged.
    fn failed(&mut self, e: &ClientError, log: &dyn Fn(&str)) {
        let text = format!("controller: {e}");
        if self.logged.as_ref() != Some(&text) {
            log(&text);
            self.logged = Some(text);
        }
    }
}

/// Forwards the events of `identity`'s watch to `feed` until the feed is gone, dialling again
/// after every drop with backoff. `client` acts as `identity`.
///
/// The backoff resets only once a backlog is caught up, so a controller that accepts the
/// hello and then fails every watch is not dialled in a tight loop. A failure is logged only
/// when its text differs from the previous one, so a controller that is down or speaks
/// another protocol shows one line per cause, not one per attempt.
pub(crate) fn read_events(
    client: &Client,
    identity: &Identity,
    feed: &Sender<Tagged>,
    log: &dyn Fn(&str),
    sleep: &dyn Fn(Duration),
) {
    let mut dialer = Dialer {
        backoff: Backoff::new(),
        logged: None,
    };
    loop {
        let ended = client
            .watch(0, WatchFilter::default(), false)
            .map_err(Ended::Failed)
            .and_then(|backlog| forward(client, identity, backlog, feed, &mut dialer));
        match ended {
            Ok(()) => {}
            Err(Ended::FeedGone) => return,
            Err(Ended::Failed(e)) => dialer.failed(&e, log),
        }
        sleep(dialer.backoff.next());
    }
}

/// Tells the feed a connection starts, forwards the backlog, then follows live events from the cursor its caught-up frame names.
/// A backlog that closes without a caught-up frame ends the connection.
fn forward(
    client: &Client,
    identity: &Identity,
    backlog: Watch,
    feed: &Sender<Tagged>,
    dialer: &mut Dialer,
) -> Result<(), Ended> {
    let send = |item| {
        feed.send((identity.clone(), item))
            .map_err(|_| Ended::FeedGone)
    };
    send(Watched::Connected)?;
    let mut caught_up = None;
    for item in backlog {
        let item = item.map_err(Ended::Failed)?;
        if let WatchItem::CaughtUp { cursor } = item {
            caught_up = Some(cursor);
            dialer.healthy();
        }
        send(Watched::Item(item))?;
    }
    let Some(cursor) = caught_up else {
        return Ok(());
    };
    let live = client
        .watch(cursor, WatchFilter::default(), true)
        .map_err(Ended::Failed)?;
    for item in live {
        send(Watched::Item(item.map_err(Ended::Failed)?))?;
    }
    Ok(())
}

#[cfg(test)]
mod tool_args_tests {
    #![allow(clippy::unwrap_used, reason = "test setup")]

    use std::path::{Path, PathBuf};

    use moor_client::tls::TlsPki;

    use super::Dial;

    #[test]
    fn a_tls_dial_names_its_files_and_never_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let key_file = dir.path().join("front.key");
        std::fs::write(&key_file, "the-front-key").unwrap();
        let dial = Dial::Tls {
            url: "tls://ledger:7443".to_owned(),
            pki: TlsPki {
                ca: PathBuf::from("/ca.pem"),
                cert: PathBuf::from("/client.pem"),
                key: PathBuf::from("/client.key"),
            },
            key_file: key_file.clone(),
        };
        let args = dial.tool_args(Path::to_owned);
        assert_eq!(
            args.get(..2).unwrap(),
            ["--controller", "tls://ledger:7443"]
        );
        assert!(args.contains(&key_file.display().to_string()));
        assert!(
            !args.iter().any(|a| a.contains("the-front-key")),
            "{args:?}"
        );
    }

    #[test]
    fn a_socket_dial_names_the_path_another_process_dials() {
        let dial = Dial::Socket(PathBuf::from("/proc/self/fd/3"));
        let args = dial.tool_args(|_| PathBuf::from("/proc/42/fd/3"));
        assert_eq!(args, ["--socket", "/proc/42/fd/3"]);
    }

    #[test]
    fn a_key_file_others_can_read_gives_no_client() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let key_file = dir.path().join("front.key");
        std::fs::write(&key_file, "k").unwrap();
        std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let dial = Dial::Tls {
            url: "tls://ledger:7443".to_owned(),
            pki: TlsPki {
                ca: dir.path().join("none.pem"),
                cert: dir.path().join("none.pem"),
                key: dir.path().join("none.key"),
            },
            key_file,
        };
        let err = dial.client().err().unwrap();
        assert!(err.contains("must be mode 0600"), "{err}");
    }
}
