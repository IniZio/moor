//! The daemon: loads the config and tokens, then runs the front's five threads.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use moor_client::{Client, WatchItem};
use moor_core::secret::Secret;

use crate::api::Api;
use crate::config::{parse_config, Config};
use crate::secret_file::read_secret;
use crate::socket::{self, Inbound};
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
    /// Home directory that `~` in token paths expands to.
    pub home: Option<PathBuf>,
}

/// The config and Slack tokens, read and checked before any thread starts.
struct Loaded {
    /// Web API client rooted at the configured base.
    api: Api,
    /// App-level token for Socket Mode.
    app_token: Secret,
}

/// Runs the daemon until its Socket Mode thread ends.
///
/// # Errors
/// Returns why startup failed: an unreadable or invalid config, a token file that is
/// missing, empty or readable by others, or an unusable API base. Never carries a token.
pub fn run(settings: &Settings, log: Log) -> Result<(), String> {
    let loaded = load(settings)?;
    let client = Client::new(&settings.socket);
    let (inbound_tx, inbound_rx) = mpsc::channel();
    let (feed_tx, feed_rx) = mpsc::channel();
    let (wake_tx, wake_rx) = mpsc::channel();
    let socket = thread::spawn(move || {
        socket::run(&loaded.api, &loaded.app_token, &inbound_tx, &thread::sleep);
    });
    thread::spawn(move || work(&inbound_rx));
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
    let (app_token, _bot_token) = read_tokens(&cfg, settings.home.as_deref())?;
    let api = Api::new(&cfg.api_base).map_err(|e| format!("slack: {e}"))?;
    Ok(Loaded { api, app_token })
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

/// The worker thread: takes queued mentions and clicks one at a time.
fn work(inbound: &Receiver<Inbound>) {
    for _item in inbound {}
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
