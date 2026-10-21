//! The daemon: loads the config and tokens, then runs the front's five threads.

use std::ffi::OsString;
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use moor_client::Client;
use moor_core::secret::Secret;

use crate::api::Api;
use crate::command::Allow;
use crate::config::{parse_config, Config, CoordinatorAgent};
use crate::coordinator::claude::{Claude, HostEnv};
use crate::coordinator::Agent;
use crate::credentials::CredentialCheck;
use crate::db::{Db, SlackBox};
use crate::feed::Feed;
use crate::login::{self, TokenCheck};
use crate::secret_file::read_secret;
use crate::socket::{self, AfterAck, Inbound, Intake, Kept, NotKept};
use crate::turns::Turns;
use crate::worker::{self, Fx, Job};
use crate::{dial, slash, Log};

/// What the daemon needs from its command line and environment.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Controller socket to dial.
    pub socket: PathBuf,
    /// The front's config file.
    pub config: PathBuf,
    /// Directory for the front's own state, `slack.db`.
    pub state_dir: PathBuf,
    /// Home directory that `~` in token paths expands to; also `HOME` for the coordinator.
    pub home: Option<PathBuf>,
    /// `USER` for the coordinator.
    pub user: Option<String>,
    /// The daemon's `PATH`, where the coordinator agent looks for its binary when the config
    /// names none.
    pub path: Option<OsString>,
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
    /// Bot token for posting replies and opening modals.
    bot_token: Arc<Secret>,
    /// The checked coordinator agent, or why it cannot run.
    coordinator: Result<Arc<dyn Agent>, String>,
    /// `slack.db`, opened at this build's schema; the Socket Mode thread records mentions in it.
    db: Db,
    /// Path of `slack.db`, which the worker opens again on its own thread.
    db_path: PathBuf,
    /// Boxes an earlier run left `starting` or `closing`, read before any new mention is
    /// recorded.
    left: Vec<SlackBox>,
    /// The coordinator's turns.
    turns: Turns,
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
        coordinator,
        db,
        db_path,
        left,
        turns,
    } = load(settings, &log)?;
    let client = Client::new(&settings.socket);
    let (jobs_tx, jobs_rx) = mpsc::channel();
    let (feed_tx, feed_rx) = mpsc::channel();
    let (_wake_tx, wake_rx) = mpsc::channel();
    let recorder = Recorder {
        db,
        channels: cfg.channels,
        users: cfg.users,
        state_dir: settings.state_dir.clone(),
        api: Arc::clone(&api),
        bot_token: Arc::clone(&bot_token),
        jobs: jobs_tx,
        log: Arc::clone(&log),
        clock: settings.clock,
    };
    let socket_api = Arc::clone(&api);
    let socket = thread::spawn(move || {
        socket::run(&socket_api, &app_token, &recorder, &thread::sleep);
    });
    let feed = Feed {
        db_path: db_path.clone(),
        api: Arc::clone(&api),
        bot_token: Arc::clone(&bot_token),
        log: Arc::clone(&log),
        clock: settings.clock,
    };
    let worker = Worker {
        db_path,
        state_dir: settings.state_dir.clone(),
        api,
        bot_token,
        coordinator,
        client: client.clone(),
        log: Arc::clone(&log),
    };
    thread::spawn(move || worker.run(&jobs_rx, left));
    thread::spawn(move || Arc::new(turns).run());
    thread::spawn(move || dial::read_events(&client, &feed_tx, &*log, &thread::sleep));
    thread::spawn(move || feed.run(&feed_rx));
    thread::spawn(move || post(&wake_rx));
    socket
        .join()
        .map_err(|_| "the Socket Mode thread panicked".to_owned())?;
    Err("the worker stopped taking mentions".to_owned())
}

/// Reads the config and both token files, and opens `slack.db`, so a bad file stops startup.
/// A missing or old Claude Code does not stop it; the coordinator is then unavailable.
fn load(settings: &Settings, log: &Log) -> Result<Loaded, String> {
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
    let db = Db::open(&db_path).map_err(|e| format!("open slack.db: {e}"))?;
    let read = |e: rusqlite::Error| format!("read slack.db: {e}");
    let mut left = db.starting().map_err(read)?;
    left.extend(db.closing().map_err(read)?);
    let coordinator = probe(&cfg, settings, log);
    let api = Arc::new(api);
    let bot_token = Arc::new(bot_token);
    let program = std::env::current_exe().map_err(|e| format!("find this binary: {e}"))?;
    let socket = std::path::absolute(&settings.socket)
        .map_err(|e| format!("{}: {e}", settings.socket.display()))?;
    let c = &cfg.coordinator;
    let turns = Turns {
        db_path: db_path.clone(),
        state_dir: settings.state_dir.clone(),
        agent: coordinator.clone(),
        model: c.model.clone(),
        budget: usize::try_from(c.context_kib).map_or(usize::MAX, |kib| kib.saturating_mul(1024)),
        api: Arc::clone(&api),
        bot_token: Arc::clone(&bot_token),
        client: Client::new(&settings.socket),
        socket,
        program,
        max_open_boxes: c.max_open_boxes,
        idle_secs: i64::from(c.idle_minutes) * 60,
        log: Arc::clone(log),
        clock: settings.clock,
    };
    Ok(Loaded {
        cfg,
        api,
        app_token,
        bot_token,
        coordinator,
        db,
        db_path,
        left,
        turns,
    })
}

/// The coordinator agent the config names, probed and logged, or why it cannot run.
fn probe(cfg: &Config, settings: &Settings, log: &Log) -> Result<Arc<dyn Agent>, String> {
    let host = HostEnv {
        home: settings.home.clone(),
        user: settings.user.clone(),
    };
    let c = &cfg.coordinator;
    let binary = c.binary.as_deref();
    let coordinator: Result<Arc<dyn Agent>, String> = match c.agent {
        CoordinatorAgent::Claude => Claude::locate(binary, settings.path.as_deref(), host)
            .map(|a| Arc::new(a) as Arc<dyn Agent>),
    };
    match &coordinator {
        Ok(agent) => log(&format!(
            "coordinator: {} {}",
            agent.program(),
            agent.version()
        )),
        Err(why) => log(&format!("coordinator unavailable: {why}")),
    }
    coordinator
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

/// The Socket Mode thread's intake: records each mention and command in `slack.db`, then
/// queues its job, all before Slack gets the ack.
struct Recorder {
    /// The Socket Mode thread's own `slack.db` connection.
    db: Db,
    /// Channel ids where mentions are answered.
    channels: Vec<String>,
    /// User ids who may start Boxes.
    users: Vec<String>,
    /// The front's state dir, which holds the token files.
    state_dir: PathBuf,
    /// Web API client, for the login modal.
    api: Arc<Api>,
    /// Bot token for opening the login modal.
    bot_token: Arc<Secret>,
    /// The worker's queue.
    jobs: Sender<Job>,
    /// Daemon log.
    log: Log,
    /// The current time in unix seconds.
    clock: fn() -> i64,
}

impl Intake for Recorder {
    /// Keeps a mention, a command or a login submission; a click needs nothing kept until
    /// prompts take answers.
    fn take(&self, item: Inbound) -> Result<Kept, NotKept> {
        let allow = Allow {
            channels: &self.channels,
            users: &self.users,
        };
        let now = (self.clock)();
        let kept = match item {
            Inbound::Mention(mention) => {
                worker::record(&mention, &self.db, &allow, now).map(|job| (Kept::default(), job))
            }
            Inbound::Slash(slash) => {
                let state = &self.state_dir;
                slash::record(&slash, &self.db, &allow, state, now, &*self.log)
            }
            Inbound::Submission(sub) => {
                login::submission(sub, &self.db, &self.users, now).map(|(ack, job)| {
                    (
                        Kept {
                            ack: Some(ack),
                            then: None,
                        },
                        job,
                    )
                })
            }
            Inbound::Click(_) => return Ok(Kept::default()),
        };
        match kept {
            Ok((kept, Some(job))) => self
                .jobs
                .send(job)
                .map(|()| kept)
                .map_err(|_| NotKept::Gone),
            Ok((kept, None)) => Ok(kept),
            Err(e) => {
                (self.log)(&format!("slack.db: {e}; Slack sends the envelope again"));
                Err(NotKept::Failed)
            }
        }
    }

    /// Opens the login modal on the Socket Mode thread, because its trigger expires 3 s
    /// after the command and the worker may be busy with a Box start.
    fn after_ack(&self, then: AfterAck) {
        let AfterAck::OpenLogin { trigger_id, agent } = then;
        let form = login::form(agent);
        if let Err(e) = self.api.open_view(&self.bot_token, &trigger_id, &form) {
            (self.log)(&format!("slack: login modal failed: {e}"));
        }
    }
}

/// The worker thread's state: where `slack.db` lives, Slack and the controller.
struct Worker {
    /// Path of `slack.db`; the worker opens its own connection.
    db_path: PathBuf,
    /// The front's state dir, which holds the token files.
    state_dir: PathBuf,
    /// Web API client.
    api: Arc<Api>,
    /// Bot token for posting replies.
    bot_token: Arc<Secret>,
    /// The checked coordinator agent, or why it cannot run.
    coordinator: Result<Arc<dyn Agent>, String>,
    /// Controller socket client; each request opens its own connection.
    client: Client,
    /// Daemon log.
    log: Log,
}

impl Worker {
    /// Settles the rows `left` by an earlier run, runs the queued jobs and retries unsure
    /// starts until the queue closes.
    fn run(&self, jobs: &Receiver<Job>, left: Vec<SlackBox>) {
        let db = match Db::open(&self.db_path) {
            Ok(db) => db,
            Err(e) => return (self.log)(&format!("open slack.db: {e}")),
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
        let mut say = |channel: &str, text: &str| {
            if let Err(e) = self.api.post_in_channel(&self.bot_token, channel, text) {
                (self.log)(&format!("slack: post failed: {e}"));
            }
        };
        let mut check_token = |check: TokenCheck| {
            let checker = self.coordinator.as_ref();
            let checker = checker
                .map(|c| &**c as &dyn CredentialCheck)
                .map_err(String::as_str);
            let text = login::check(&check, checker, &self.state_dir);
            (self.log)(&format!("login of {}: {text}", check.user.as_str()));
            let view = login::notice(&text);
            if let Err(e) = self.api.update_view(&self.bot_token, &check.view_id, &view) {
                (self.log)(&format!("slack: login modal update failed: {e}"));
            }
        };
        let mut fx = Fx {
            call: &mut call,
            reply: &mut reply,
            say: &mut say,
            check_token: &mut check_token,
            log: &*self.log,
        };
        worker::serve(jobs, &db, left, &mut fx);
    }
}

/// The poster thread: waits for the feed to wake it.
fn post(wake: &Receiver<()>) {
    for () in wake {}
}
