//! The daemon: loads the config and tokens, then runs the front's five threads.

use std::ffi::OsString;
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;

use moor_client::Client;
use moor_core::secret::Secret;

use crate::api::{Api, BotIdentity};
use crate::config::{parse_config, Config, CoordinatorAgent};
use crate::coordinator::claude::{Claude, HostEnv};
use crate::coordinator::Agent;
use crate::credentials::CredentialCheck;
use crate::db::{Db, SlackBox};
use crate::feed::Feed;
use crate::github::GitHub;
use crate::identity::{Link, Team};
use crate::login::{self, Checks, TokenCheck};
use crate::secret_file::read_secret;
use crate::socket;
use crate::turns::{FrontProxy, Turns};
use crate::worker::{self, Fx, Job};
use crate::Log;

mod recorder;
mod roleplay;
use recorder::Recorder;
use roleplay::roleplay;
mod stop;
use stop::{finish, sleep_until_stopped};

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
    /// The loopback port of the front Proxy, which the front's shim bridges.
    pub proxy_port: Option<u16>,
    /// The PEM certificate of the CA the front Proxy signs with.
    pub proxy_ca_pem: Option<String>,
    /// The variables that name the egress proxy the front shim gave the daemon, such as
    /// `HTTPS_PROXY` and `NO_PROXY`, with their values; empty where the front reaches the
    /// internet directly.
    pub egress_proxy: Vec<(String, OsString)>,
    /// The current time in unix seconds.
    pub clock: fn() -> i64,
    /// Set when the daemon is asked to stop: it stops acking events, finishes the request in
    /// flight and returns `Ok`.
    pub stop: Arc<AtomicBool>,
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
    /// The controller's front socket and the Slack team.
    link: Link,
    /// Who the bot may post as, when the config turns role-play on.
    roleplay: Option<socket::Roleplay>,
}

/// Runs the daemon until its Socket Mode thread ends, or until `settings.stop` is set and the
/// worker has finished the request in flight.
///
/// # Errors
/// Returns why startup failed: an unreadable or invalid config, a token file that is
/// missing, empty or readable by others, an unusable API base, or a state directory or
/// `slack.db` that cannot be opened. Never carries a token.
pub fn run(settings: &Settings, log: &Log) -> Result<(), String> {
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
        link,
        roleplay,
    } = load(settings, log)?;
    let (jobs_tx, jobs_rx) = mpsc::channel();
    let (_wake_tx, wake_rx) = mpsc::channel();
    let recorder = Recorder {
        db,
        api: Arc::clone(&api),
        bot_token: Arc::clone(&bot_token),
        jobs: jobs_tx,
        log: Arc::clone(log),
        clock: settings.clock,
    };
    let socket_api = Arc::clone(&api);
    let stop = Arc::clone(&settings.stop);
    let socket = thread::spawn(move || {
        let sleep = |d| sleep_until_stopped(&stop, d);
        socket::run(
            &socket_api,
            &app_token,
            &recorder,
            roleplay.as_ref(),
            &stop,
            &sleep,
        );
    });
    let feed = Feed {
        link: link.clone(),
        db_path: db_path.clone(),
        api: Arc::clone(&api),
        bot_token: Arc::clone(&bot_token),
        log: Arc::clone(log),
        clock: settings.clock,
        long_turn_secs: i64::from(cfg.long_turn_minutes) * 60,
    };
    let worker = Worker {
        db_path,
        state_dir: settings.state_dir.clone(),
        api,
        bot_token,
        coordinator,
        github: GitHub::new(&cfg.github_api_base),
        link,
        log: Arc::clone(log),
    };
    let worker_thread = thread::spawn(move || worker.run(&jobs_rx, left));
    thread::spawn(move || Arc::new(turns).run());
    thread::spawn(move || feed.run());
    thread::spawn(move || post(&wake_rx));
    finish(&settings.stop, socket, worker_thread)
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
    let api = Arc::new(api);
    let (link, me) = connect(settings, &api, &bot_token)?;
    let dir = &settings.state_dir;
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("create {}: {e}", dir.display()))?;
    let db_path = dir.join("slack.db");
    let db = Db::open(&db_path).map_err(|e| format!("open slack.db: {e}"))?;
    let read = |e: rusqlite::Error| format!("read slack.db: {e}");
    reopen_unsent(&db, log)?;
    let mut left = db.starting().map_err(read)?;
    left.extend(db.closing().map_err(read)?);
    let coordinator = probe(&cfg, settings, log);
    let bot_token = Arc::new(bot_token);
    let roleplay = roleplay(&cfg, &me, &api, &bot_token, log)?;
    let program = std::env::current_exe().map_err(|e| format!("find this binary: {e}"))?;
    let socket = std::path::absolute(&settings.socket)
        .map(|path| own_proc(&path))
        .map_err(|e| format!("{}: {e}", settings.socket.display()))?;
    let c = &cfg.coordinator;
    let turns = Turns {
        db_path: db_path.clone(),
        state_dir: settings.state_dir.clone(),
        agent: coordinator.clone(),
        proxy: FrontProxy::written(dir, settings.proxy_port, settings.proxy_ca_pem.as_deref())?,
        model: c.model.clone(),
        budget: usize::try_from(c.context_kib).map_or(usize::MAX, |kib| kib.saturating_mul(1024)),
        api: Arc::clone(&api),
        bot_token: Arc::clone(&bot_token),
        link: link.clone(),
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
        link,
        roleplay,
    })
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

/// Opens again the prompts whose answer an earlier run never sent.
fn reopen_unsent(db: &Db, log: &Log) -> Result<(), String> {
    let reopened = db
        .reopen_unsent()
        .map_err(|e| format!("write slack.db: {e}"))?;
    if reopened > 0 {
        log(&format!(
            "slack: reopened {reopened} prompts whose answer was never sent"
        ));
    }
    Ok(())
}

/// The link to the controller's front socket for the Slack team the bot token belongs to, and
/// who the bot is.
fn connect(
    settings: &Settings,
    api: &Api,
    bot_token: &Secret,
) -> Result<(Link, BotIdentity), String> {
    let me = api
        .auth_test(bot_token)
        .map_err(|e| format!("slack auth.test: {e}"))?;
    let team = Team::parse(&me.team)?;
    Ok((Link::new(Client::new(&settings.socket), team), me))
}

/// The coordinator agent the config names, probed and logged, or why it cannot run.
fn probe(cfg: &Config, settings: &Settings, log: &Log) -> Result<Arc<dyn Agent>, String> {
    let host = HostEnv {
        home: settings.home.clone(),
        user: settings.user.clone(),
        egress_proxy: settings.egress_proxy.clone(),
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
    /// GitHub, which checks a pasted token.
    github: GitHub,
    /// The controller; each request opens its own connection and speaks for one Slack user.
    link: Link,
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
        let mut call = |user: &str, req| self.link.request(user, req);
        let mut reply = |channel: &str, thread: &str, text: &str| {
            if let Err(e) = self
                .api
                .post_message(&self.bot_token, channel, thread, text)
            {
                (self.log)(&format!("slack: reply failed: {e}"));
            }
        };
        let mut ephemeral = |channel: &str, user: &str, thread: &str, text: &str| {
            let api = &self.api;
            if let Err(e) = api.post_ephemeral(&self.bot_token, channel, user, thread, text) {
                (self.log)(&format!("slack: ephemeral failed: {e}"));
            }
        };
        let mut check_token = |check: &TokenCheck| {
            let claude = self.coordinator.as_ref();
            let claude = claude
                .map(|c| &**c as &dyn CredentialCheck)
                .map_err(String::as_str);
            let checks = Checks {
                claude,
                github: &self.github,
                state_dir: &self.state_dir,
            };
            checks.run(check)
        };
        let mut modal = |view_id: &str, text: &str| {
            let view = login::notice(text);
            if let Err(e) = self.api.update_view(&self.bot_token, view_id, &view) {
                (self.log)(&format!("slack: login modal update failed: {e}"));
            }
        };
        let mut respond = |url: &str, text: &str| {
            if let Err(e) = self.api.respond(url, text) {
                (self.log)(&format!("slack: command answer failed: {e}"));
            }
        };
        let mut fx = Fx {
            call: &mut call,
            reply: &mut reply,
            ephemeral: &mut ephemeral,
            check_token: &mut check_token,
            modal: &mut modal,
            respond: &mut respond,
            log: &*self.log,
        };
        worker::serve(jobs, &db, left, &mut fx);
    }
}

/// The poster thread: waits for the feed to wake it.
fn post(wake: &Receiver<()>) {
    for () in wake {}
}
