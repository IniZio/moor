//! Slack Socket Mode front end: an app mention in an allowed channel starts a Task.

mod answer;
mod api;
mod command;
mod pr;
mod prompt;
mod proof;
mod socket;
mod status;

use std::net::SocketAddr;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;

use moor_core::config::SlackConfig;
use moor_core::request::{CloseReq, Request, StartReq, SteerReq};
use moor_core::secret::Secret;
use serde_json::Value;

use crate::adapters::clock::SystemClock;
use crate::env::Env;
use crate::store::{Store, STORE_FILE};
use crate::Moor;
use answer::{answer_open, handle_click, Fx, Ui};
use api::Api;
use command::{decide, failed, reply_action, started, Action, Allow, ReplyAction};
pub(crate) use pr::PrOf;
use socket::{Inbound, Mention};

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
    /// Bot token for the status follower, which runs on its own thread.
    status_token: Secret,
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
            status_token: Secret::new(bot.clone().into_bytes()),
            bot_token: Secret::new(bot.into_bytes()),
            channels: cfg.channels.clone(),
            users: cfg.users.clone(),
        })
    }

    /// Starts the socket thread, the worker and the status follower for the life of the process.
    ///
    /// Starting a Task can take minutes, so the socket thread only queues mentions.
    /// `web` is the controller's web address, used for links in status messages.
    /// `pr_of` finds the open pull request of a Task's branch for the same messages.
    pub(crate) fn spawn(self, env: &Env, log: Log, web: Option<SocketAddr>, pr_of: Arc<PrOf>) {
        let (tx, rx) = mpsc::channel();
        let api = Arc::new(self.api);
        let socket_api = Arc::clone(&api);
        let app_token = self.app_token;
        let status_api = Arc::clone(&api);
        let status_env = env.clone();
        let status_token = self.status_token;
        let status_log = Arc::clone(&log);
        thread::spawn(move || {
            status::follow(
                &status_env,
                &status_api,
                &status_token,
                web,
                &*pr_of,
                &*status_log,
            );
        });
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

/// Handles queued mentions and button presses one at a time.
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
    /// Serves queued events until the socket thread is gone.
    fn run(&self, inbound: &Receiver<Inbound>) {
        let allow = Allow {
            channels: &self.channels,
            users: &self.users,
        };
        for item in inbound {
            let store = match self.open_store() {
                Ok(store) => store,
                Err(e) => {
                    (self.log)(&format!("slack: {e}"));
                    continue;
                }
            };
            let mut call = |req: Request| self.call(req);
            let mut reply = |channel: &str, thread: &str, text: &str| {
                if let Err(e) = self
                    .api
                    .post_message(&self.bot_token, channel, thread, text)
                {
                    (self.log)(&format!("slack: reply failed: {e}"));
                }
            };
            let mut ui = ApiUi(self);
            let mut fx = Fx {
                call: &mut call,
                reply: &mut reply,
                ui: &mut ui,
                log: &*self.log,
            };
            match item {
                Inbound::Mention(mention) => handle(&mention, &store, &allow, &mut fx),
                Inbound::Click(click) => handle_click(&click, &store, &mut fx),
            }
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

    /// Runs `req` on a fresh engine; `Moor` is not `Send`.
    fn call(&self, req: Request) -> Result<(), String> {
        let moor = Moor::from_env(&self.env).map_err(|e| e.error.to_string())?;
        let response = moor.call(req);
        response.error.map_or(Ok(()), |e| Err(e.message))
    }
}

/// Sends ephemeral messages and message edits through the Web API.
struct ApiUi<'a>(&'a Worker);

impl Ui for ApiUi<'_> {
    fn ephemeral(&mut self, channel: &str, user: &str, thread: &str, text: &str) {
        let w = self.0;
        if let Err(e) = w
            .api
            .post_ephemeral(&w.bot_token, channel, user, thread, text)
        {
            (w.log)(&format!("slack: ephemeral failed: {e}"));
        }
    }

    fn update(&mut self, channel: &str, ts: &str, text: &str, blocks: &Value) {
        let w = self.0;
        if let Err(e) = w.api.update_blocks(&w.bot_token, channel, ts, text, blocks) {
            (w.log)(&format!("slack: update failed: {e}"));
        }
    }
}

/// Applies one event: dedupe, then steer or close a bound Task, or decide, start and bind.
fn handle(mention: &Mention, store: &Store, allow: &Allow<'_>, fx: &mut Fx<'_>) {
    match store.slack_event_once(&mention.event_id) {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => return (fx.log)(&format!("slack: {e}")),
    }
    let channel = mention.channel.as_str();
    if mention.reply {
        return handle_reply(mention, store, allow, fx);
    }
    let thread = mention.thread_ts.as_deref().unwrap_or(&mention.ts);
    let bound = match store.slack_thread(channel, thread) {
        Ok(bound) => bound,
        Err(e) => return (fx.log)(&format!("slack: {e}")),
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
        Action::Reply(text) => (fx.reply)(channel, thread, &text),
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
            match (fx.call)(Request::Start(req)) {
                Ok(()) => {
                    if let Err(e) = store.bind_slack_thread(channel, thread, &name, &mention.user) {
                        (fx.log)(&format!("slack: {e}"));
                    }
                    (fx.reply)(channel, thread, &started(&name));
                }
                Err(message) => (fx.reply)(channel, thread, &failed(&name, &message)),
            }
        }
    }
}

/// Applies a plain thread reply: the Task owner answers an open prompt, steers or closes;
/// anyone else is refused.
fn handle_reply(mention: &Mention, store: &Store, allow: &Allow<'_>, fx: &mut Fx<'_>) {
    let channel = mention.channel.as_str();
    let Some(thread) = mention.thread_ts.as_deref() else {
        return;
    };
    if !allow.channels.iter().any(|c| c == channel) {
        return;
    }
    let found = store.slack_thread(channel, thread).and_then(|task| {
        let owner = store.slack_thread_owner(channel, thread)?;
        Ok(task.zip(owner))
    });
    let (task, owner) = match found {
        Ok(Some(pair)) => pair,
        Ok(None) => return,
        Err(e) => return (fx.log)(&format!("slack: {e}")),
    };
    if mention.user == owner {
        if let Some(choice) = bare_number(&mention.text) {
            if answer_open(channel, thread, &owner, choice, store, fx) {
                return;
            }
        }
    }
    let result = match reply_action(&owner, &mention.user, &mention.text) {
        ReplyAction::Ignore => return,
        ReplyAction::Refuse(text) => return (fx.reply)(channel, thread, &text),
        ReplyAction::Steer(text) => (fx.call)(Request::Steer(SteerReq {
            task,
            text: Some(text),
            menu: None,
        })),
        ReplyAction::Close => (fx.call)(Request::Close(CloseReq {
            task,
            force: false,
            trust_moor_yaml: false,
        })),
    };
    if let Err(message) = result {
        (fx.reply)(channel, thread, &message);
    }
}

/// The positive number a reply consists of, ignoring user mentions.
fn bare_number(text: &str) -> Option<u32> {
    let mut words = text.split_whitespace().filter(|w| !w.starts_with("<@"));
    let n = words.next()?.parse::<u32>().ok()?;
    (words.next().is_none() && n > 0).then_some(n)
}

#[cfg(test)]
mod tests;
