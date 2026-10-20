//! The daemon: loads the config and tokens, then runs the front's five threads.

use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_client::{Client, WatchItem};
use moor_core::secret::Secret;

use crate::api::Api;
use crate::command::Allow;
use crate::config::{parse_config, Config};
use crate::db::Db;
use crate::secret_file::read_secret;
use crate::socket::{self, Inbound};
use crate::worker::{self, Fx};
use crate::{dial, Log};

/// Longest the feed waits for a watch item before it wakes anyway.
const FEED_TICK: Duration = Duration::from_millis(500);

/// What the daemon needs from its command line and environment.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Controller socket to dial.
    pub socket: PathBuf,
    /// The front's config file.
    pub config: PathBuf,
    /// Directory for the front's own state, `slack.db`.
    pub state_dir: PathBuf,
    /// Home directory that `~` in token paths expands to.
    pub home: Option<PathBuf>,
    /// The current time in unix seconds.
    pub clock: fn() -> i64,
}

/// The config, Slack tokens and `slack.db`, read and checked before any thread starts.
struct Loaded {
    /// The parsed config.
    cfg: Config,
    /// Web API client rooted at the configured base.
    api: Arc<Api>,
    /// App-level token for Socket Mode.
    app_token: Secret,
    /// Bot token for posting replies.
    bot_token: Secret,
    /// Path of `slack.db`, opened and migrated once to check it.
    db_path: PathBuf,
}

/// Runs the daemon until its Socket Mode thread ends.
///
/// # Errors
/// Returns why startup failed: an unreadable or invalid config, a token file that is
/// missing, empty or readable by others, an unusable API base, or a state directory or
/// `slack.db` that cannot be opened. Never carries a token.
pub fn run(settings: &Settings, log: Log) -> Result<(), String> {
    let Loaded {
        cfg,
        api,
        app_token,
        bot_token,
        db_path,
    } = load(settings)?;
    let client = Client::new(&settings.socket);
    let (inbound_tx, inbound_rx) = mpsc::channel();
    let (feed_tx, feed_rx) = mpsc::channel();
    let (wake_tx, wake_rx) = mpsc::channel();
    let socket_api = Arc::clone(&api);
    let socket = thread::spawn(move || {
        socket::run(&socket_api, &app_token, &inbound_tx, &thread::sleep);
    });
    let worker = Worker {
        db_path,
        api,
        bot_token,
        client: client.clone(),
        channels: cfg.channels,
        users: cfg.users,
        log: Arc::clone(&log),
        clock: settings.clock,
    };
    thread::spawn(move || worker.run(&inbound_rx));
    thread::spawn(move || dial::read_events(&client, &feed_tx, &*log, &thread::sleep));
    thread::spawn(move || feed(&feed_rx, &wake_tx));
    thread::spawn(move || post(&wake_rx));
    socket
        .join()
        .map_err(|_| "the Socket Mode thread panicked".to_owned())?;
    Err("the worker stopped taking mentions".to_owned())
}

/// Reads the config and both token files, so a bad file stops startup.
fn load(settings: &Settings) -> Result<Loaded, String> {
    let path = &settings.config;
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let cfg = parse_config(&path.display().to_string(), &text).map_err(|e| e.to_string())?;
    let (app_token, bot_token) = read_tokens(&cfg, settings.home.as_deref())?;
    let api = Api::new(&cfg.api_base).map_err(|e| format!("slack: {e}"))?;
    let dir = &settings.state_dir;
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("create {}: {e}", dir.display()))?;
    let db_path = dir.join("slack.db");
    Db::open(&db_path).map_err(|e| format!("open slack.db: {e}"))?;
    Ok(Loaded {
        cfg,
        api: Arc::new(api),
        app_token,
        bot_token,
        db_path,
    })
}

/// Reads the app and bot tokens; each file must be owner-only and non-empty.
fn read_tokens(cfg: &Config, home: Option<&Path>) -> Result<(Secret, Secret), String> {
    let read = |path: &Path, label: &str| {
        read_secret(&expand_home(path, home), label).map(|t| Secret::new(t.into_bytes()))
    };
    Ok((
        read(&cfg.app_token_file, "slack app token file")?,
        read(&cfg.bot_token_file, "slack bot token file")?,
    ))
}

/// `path` with a leading `~` replaced by `home`, when both are present.
fn expand_home(path: &Path, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix("~"), home) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => path.to_path_buf(),
    }
}

/// The worker thread's state: where `slack.db` lives, Slack, the controller and the allow-list.
struct Worker {
    /// Path of `slack.db`; the worker opens its own connection.
    db_path: PathBuf,
    /// Web API client.
    api: Arc<Api>,
    /// Bot token for posting replies.
    bot_token: Secret,
    /// Controller socket client; each request opens its own connection.
    client: Client,
    /// Channel ids where mentions are answered.
    channels: Vec<String>,
    /// User ids who may start Boxes.
    users: Vec<String>,
    /// Daemon log.
    log: Log,
    /// The current time in unix seconds.
    clock: fn() -> i64,
}

impl Worker {
    /// Settles rows left in `starting`, then takes queued mentions one at a time.
    fn run(&self, inbound: &Receiver<Inbound>) {
        let db = match Db::open(&self.db_path) {
            Ok(db) => db,
            Err(e) => return (self.log)(&format!("open slack.db: {e}")),
        };
        let allow = Allow {
            channels: &self.channels,
            users: &self.users,
        };
        let mut call = |req| self.client.request(req, |_| {});
        let mut reply = |channel: &str, thread: &str, text: &str| {
            if let Err(e) = self
                .api
                .post_message(&self.bot_token, channel, thread, text)
            {
                (self.log)(&format!("slack: reply failed: {e}"));
            }
        };
        let mut fx = Fx {
            call: &mut call,
            reply: &mut reply,
            log: &*self.log,
            wait: &thread::sleep,
            now: self.clock,
        };
        worker::resume(&db, &mut fx);
        for item in inbound {
            if let Inbound::Mention(mention) = item {
                worker::handle(&mention, &db, &allow, &mut fx);
            }
        }
    }
}

/// The feed thread: takes watch items as they arrive and wakes at least every [`FEED_TICK`].
fn feed(items: &Receiver<WatchItem>, _wake_poster: &Sender<()>) {
    loop {
        match items.recv_timeout(FEED_TICK) {
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// The poster thread: waits for the feed to wake it.
fn post(wake: &Receiver<()>) {
    for () in wake {}
}
