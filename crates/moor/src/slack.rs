//! Slack Socket Mode front end: an app mention in an allowed channel starts a Task.

mod api;
mod command;
mod socket;

use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;

use moor_core::config::SlackConfig;
use moor_core::request::{Request, StartReq};
use moor_core::secret::Secret;

use crate::adapters::clock::SystemClock;
use crate::env::Env;
use crate::store::{Store, STORE_FILE};
use crate::Moor;
use api::Api;
use command::{decide, failed, started, Action, Allow};
use socket::Mention;

/// Diagnostic sink for the controller log.
pub(crate) type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// A configured Slack app, ready to start once the controller is serving.
pub(crate) struct Slack {
    /// Web API client.
    api: Api,
    /// App-level token for Socket Mode.
    app_token: Secret,
    /// Bot token for posting replies.
    bot_token: Secret,
    /// Channel ids where mentions are answered.
    channels: Vec<String>,
    /// User ids who may start Tasks.
    users: Vec<String>,
}

impl Slack {
    /// Builds the app from `cfg` and the token texts read on this host.
    ///
    /// # Errors
    /// Returns the client error text when the API base is unusable.
    pub(crate) fn new(cfg: &SlackConfig, app: String, bot: String) -> Result<Self, String> {
        Ok(Self {
            api: Api::new(&cfg.api_base).map_err(|e| e.to_string())?,
            app_token: Secret::new(app.into_bytes()),
            bot_token: Secret::new(bot.into_bytes()),
            channels: cfg.channels.clone(),
            users: cfg.users.clone(),
        })
    }

    /// Starts the socket thread and the worker for the life of the process.
    ///
    /// Starting a Task can take minutes, so the socket thread only queues mentions.
    pub(crate) fn spawn(self, env: &Env, log: Log) {
        let (tx, rx) = mpsc::channel();
        let api = Arc::new(self.api);
        let socket_api = Arc::clone(&api);
        let app_token = self.app_token;
        thread::spawn(move || socket::run(&socket_api, &app_token, &tx, &thread::sleep));
        let worker = Worker {
            env: env.clone(),
            api,
            bot_token: self.bot_token,
            channels: self.channels,
            users: self.users,
            log,
        };
        thread::spawn(move || worker.run(&rx));
    }
}

/// Handles queued mentions one at a time.
struct Worker {
    /// Environment for opening the store and the engine.
    env: Env,
    /// Web API client.
    api: Arc<Api>,
    /// Bot token for posting replies.
    bot_token: Secret,
    /// Channel ids where mentions are answered.
    channels: Vec<String>,
    /// User ids who may start Tasks.
    users: Vec<String>,
    /// Controller log.
    log: Log,
}

impl Worker {
    /// Serves mentions until the socket thread is gone.
    fn run(&self, mentions: &Receiver<Mention>) {
        let allow = Allow {
            channels: &self.channels,
            users: &self.users,
        };
        for mention in mentions {
            let store = match self.open_store() {
                Ok(store) => store,
                Err(e) => {
                    (self.log)(&format!("slack: {e}"));
                    continue;
                }
            };
            let mut start = |req: &StartReq| self.start(req);
            let mut reply = |channel: &str, thread: &str, text: &str| {
                if let Err(e) = self
                    .api
                    .post_message(&self.bot_token, channel, thread, text)
                {
                    (self.log)(&format!("slack: reply failed: {e}"));
                }
            };
            handle(&mention, &store, &allow, &mut start, &mut reply, &*self.log);
        }
    }

    /// Opens the controller's store.
    fn open_store(&self) -> Result<Store, String> {
        let dir = self
            .env
            .state_dir()
            .ok_or("no state directory: set state_dir or MOOR_STATE_DIR")?;
        let clock = SystemClock::from_env(&self.env).map_err(|e| e.to_string())?;
        Store::open(&dir.join(STORE_FILE), Arc::new(clock)).map_err(|e| e.to_string())
    }

    /// Starts a Task on a fresh engine; `Moor` is not `Send`.
    fn start(&self, req: &StartReq) -> Result<(), String> {
        let moor = Moor::from_env(&self.env).map_err(|e| e.error.to_string())?;
        let response = moor.call(Request::Start(req.clone()));
        response.error.map_or(Ok(()), |e| Err(e.message))
    }
}

/// Applies one mention: dedupe, decide, then start and bind or reply.
fn handle(
    mention: &Mention,
    store: &Store,
    allow: &Allow<'_>,
    start: &mut dyn FnMut(&StartReq) -> Result<(), String>,
    reply: &mut dyn FnMut(&str, &str, &str),
    log: &dyn Fn(&str),
) {
    match store.slack_event_once(&mention.event_id) {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => return log(&format!("slack: {e}")),
    }
    let channel = mention.channel.as_str();
    let thread = mention.thread_ts.as_deref().unwrap_or(&mention.ts);
    let bound = match store.slack_thread(channel, thread) {
        Ok(bound) => bound,
        Err(e) => return log(&format!("slack: {e}")),
    };
    let action = decide(
        allow,
        channel,
        &mention.user,
        &mention.text,
        thread,
        bound.as_ref(),
    );
    match action {
        Action::Ignore => {}
        Action::Reply(text) => reply(channel, thread, &text),
        Action::Start { name, repo, goal } => {
            let req = StartReq {
                name: name.clone(),
                goal: Some(goal),
                issue: None,
                worktree: None,
                base: None,
                model: None,
                no_agent: false,
                resume: false,
                trust_moor_yaml: false,
                repo: Some(repo),
            };
            match start(&req) {
                Ok(()) => {
                    if let Err(e) = store.bind_slack_thread(channel, thread, &name) {
                        log(&format!("slack: {e}"));
                    }
                    reply(channel, thread, &started(&name));
                }
                Err(message) => reply(channel, thread, &failed(&name, &message)),
            }
        }
    }
}

#[cfg(test)]
mod tests;
