//! The coordinator's turns: the scheduler takes the starter's waiting messages as one turn per
//! thread, at most [`MAX_TURNS`] at once, and each turn shows a reaction on the message that
//! started it, runs the agent through the front Proxy on a lease of the starter's credential,
//! stores its line, posts it and takes the reaction off.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_client::Client;
use moor_core::secret::Secret;
use moor_core::user::Identity;

use crate::api::{Api, ApiError};
use crate::coordinator::brief::BRIEF;
use crate::coordinator::context::{self, Parts, Said};
use crate::coordinator::tools::{self, box_lines};
use crate::coordinator::turn::{self, Ended, Turn, LIMITS};
use crate::coordinator::{Agent, Answer, ProxyAccess, ToolServer, TurnSetup};
use crate::credentials::{self, SlackUser};
use crate::db::{Db, End, TurnRow};
use crate::identity::Link;
use crate::Log;

mod host;
mod idle;
mod lines;
mod proxy;
mod react;
mod restart;
mod server_args;
mod workdir;

/// Most turns that run at once.
const MAX_TURNS: usize = 4;

/// Longest the scheduler sleeps before it looks for waiting messages again.
const TICK: Duration = Duration::from_millis(500);

/// Seconds between the scheduler's sweeps for quiet threads.
const SWEEP_SECS: i64 = 60;

/// Waits before each new try of a line Slack did not take for a reason it may get over.
const BACKOFF: [Duration; 3] = [
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
];

/// Longest wait between tries, whatever `Retry-After` asks.
const MAX_WAIT: Duration = Duration::from_secs(30);

pub(crate) use proxy::FrontProxy;

/// Everything a turn needs, shared by the scheduler and the turns it runs.
pub(crate) struct Turns {
    /// Path of `slack.db`; the scheduler and each turn open their own connection.
    pub(crate) db_path: PathBuf,
    /// The front's state dir, which holds credentials, config dirs and workdirs.
    pub(crate) state_dir: PathBuf,
    /// The checked coordinator agent, or why it cannot run.
    pub(crate) agent: Result<Arc<dyn Agent>, String>,
    /// Where the agent reaches the front Proxy; none when the controller gave this front none.
    pub(crate) proxy: Option<FrontProxy>,
    /// The model each turn runs on.
    pub(crate) model: String,
    /// Most bytes of context a turn gets.
    pub(crate) budget: usize,
    /// Web API client.
    pub(crate) api: Arc<Api>,
    /// Bot token for posting.
    pub(crate) bot_token: Arc<Secret>,
    /// The controller and team; each turn speaks for the thread's owner.
    pub(crate) link: Link,
    /// Absolute path of the controller socket, for the tool server.
    pub(crate) socket: PathBuf,
    /// Absolute path of this binary, which the agent starts as the tool server.
    pub(crate) program: PathBuf,
    /// Most Boxes a thread has open at once.
    pub(crate) max_open_boxes: u32,
    /// Seconds a thread stays quiet before its host Session ends.
    pub(crate) idle_secs: i64,
    /// Daemon log.
    pub(crate) log: Log,
    /// The current time in unix seconds.
    pub(crate) clock: fn() -> i64,
}

impl Turns {
    /// Runs the scheduler: takes waiting turns while fewer than [`MAX_TURNS`] run, each on its
    /// own thread, and looks again when one ends or every [`TICK`]. It ends quiet threads'
    /// host Sessions at start and every [`SWEEP_SECS`].
    pub(crate) fn run(self: Arc<Self>) {
        let db = match Db::open(&self.db_path) {
            Ok(db) => db,
            Err(e) => return (self.log)(&format!("turns: open slack.db: {e}")),
        };
        let mut reruns = self.recover(&db);
        let (done_tx, done_rx) = mpsc::channel();
        let mut running = 0_usize;
        let mut next_sweep = i64::MIN;
        loop {
            if (self.clock)() >= next_sweep {
                self.sweep(&db);
                next_sweep = (self.clock)() + SWEEP_SECS;
            }
            while running < MAX_TURNS {
                let next = reruns
                    .pop()
                    .map_or_else(|| db.take_turn((self.clock)()), |r| Ok(Some(r)));
                match next {
                    Ok(Some(row)) => {
                        running += 1;
                        let (me, done) = (Arc::clone(&self), done_tx.clone());
                        thread::spawn(move || {
                            me.take(&row);
                            let _ = done.send(());
                        });
                    }
                    Ok(None) => break,
                    Err(e) => {
                        (self.log)(&format!("turns: slack.db: {e}"));
                        break;
                    }
                }
            }
            if done_rx.recv_timeout(TICK).is_ok() {
                running -= 1;
                running -= done_rx.try_iter().count();
            }
        }
    }

    /// Runs turn `row` to its posted line.
    fn take(&self, row: &TurnRow) {
        let db = match Db::open(&self.db_path) {
            Ok(db) => db,
            Err(e) => return (self.log)(&format!("turn {}: open slack.db: {e}", row.id)),
        };
        self.show_working(row);
        let closing = self.outcome(&db, row);
        let stored = db.store_reply(
            row.id,
            closing.end,
            &closing.line,
            closing.bytes,
            closing.cost,
        );
        self.written(stored);
        if self.post(row, &closing.line) {
            self.written(db.mark_posted(row.id, (self.clock)()));
            self.hide_working(&db, row);
        }
    }

    /// Posts `line` in the thread. A failure Slack may get over is tried again after a wait.
    /// True once the line landed.
    pub(super) fn post(&self, row: &TurnRow, line: &str) -> bool {
        let token = &self.bot_token;
        let mut waits = BACKOFF.into_iter();
        loop {
            let posted = self
                .api
                .post_message(token, &row.channel, &row.thread_ts, line);
            let Err(error) = posted else {
                return true;
            };
            (self.log)(&format!("slack: turn reply failed: {error}"));
            let wait = match &error {
                ApiError::Slack(_) => None,
                ApiError::RateLimited(asked) => waits.next().map(|w| asked.unwrap_or(w)),
                ApiError::Transport(_) => waits.next(),
            };
            let Some(wait) = wait else {
                return false;
            };
            thread::sleep(wait.min(MAX_WAIT));
        }
    }

    /// Logs a failed `slack.db` write.
    pub(super) fn written(&self, write: rusqlite::Result<()>) {
        if let Err(e) = write {
            (self.log)(&format!("turns: slack.db: {e}"));
        }
    }

    /// Runs the turn when it can run, and says how it ended.
    fn outcome(&self, db: &Db, row: &TurnRow) -> Closing {
        let agent = match &self.agent {
            Ok(agent) => Arc::clone(agent),
            Err(why) => return Closing::of(End::Failed, lines::unavailable(why)),
        };
        let ready = match self.ready(row, &*agent) {
            Ok(ready) => ready,
            Err(closing) => return closing,
        };
        let workdir = &ready.workdir;
        let client = self.link.client_as(ready.identity.clone());
        let shown = workdir.display().to_string();
        if let Err(e) = host::ensure(&client, db, row, &shown, agent.kind().name(), &*self.log) {
            let why = format!("the controller did not open a host Session: {e}");
            return Closing::of(End::Failed, lines::unavailable(&why));
        }
        let input = match self.context(db, row, &client) {
            Ok(input) => input,
            Err(e) => {
                return Closing::of(End::Failed, lines::unavailable(&format!("slack.db: {e}")))
            }
        };
        let lease = match proxy::lease(&client, &row.owner) {
            Ok(lease) => lease,
            Err(closing) => return closing,
        };
        let server = ToolServer {
            name: "moor",
            program: &self.program,
            args: &self.tool_args(row, &ready.identity),
            tools: &tools::names(),
        };
        let turn = Turn {
            setup: TurnSetup {
                model: &self.model,
                config_dir: &ready.config_dir,
                workdir,
                brief: BRIEF,
                tools: &server,
                proxy: &ProxyAccess {
                    port: ready.proxy.port,
                    ca: &ready.proxy.ca,
                    lease: &lease,
                },
            },
            input: &input,
            limits: LIMITS,
        };
        let mut spawned = |pid| self.written(db.set_pid(row.id, pid, restart::start_time(pid)));
        let ended = turn::run(&*agent, &turn, &mut spawned, &*self.log);
        proxy::release(&client, &lease);
        let mut closing = if proxy::failed_while_down(&client, &ended) {
            Closing::of(End::Failed, lines::PROXY_DOWN.to_owned())
        } else {
            self.closing(ended, row, agent.kind().label(), &lease)
        };
        closing.bytes = Some(input.len());
        closing
    }

    /// What the turn of `row` runs with: the front Proxy, the owner's directories, and the
    /// identity the controller knows them by. The turn ends here when any of it is missing.
    fn ready(&self, row: &TurnRow, agent: &dyn Agent) -> Result<Ready, Closing> {
        let unavailable = |why: &str| Closing::of(End::Failed, lines::unavailable(why));
        let proxy = self
            .proxy
            .clone()
            .ok_or_else(|| unavailable(lines::NO_PROXY))?;
        let user = SlackUser::parse(&row.owner)
            .ok_or_else(|| unavailable("the starter has no Slack user id"))?;
        let identity = self
            .link
            .identity(&row.owner)
            .map_err(|e| unavailable(&e))?;
        let config_dir = credentials::config_dir(&self.state_dir, agent.kind(), &user)
            .map_err(|e| unavailable(&e))?;
        let workdir = workdir::workdir(&self.state_dir, row).map_err(|e| unavailable(&e))?;
        if let Some(why) = agent.unsafe_cwd() {
            (self.log)(&format!("turn {}: {why}", row.id));
            return Err(Closing::of(End::Unsafe, lines::UNSAFE.to_owned()));
        }
        Ok(Ready {
            proxy,
            config_dir,
            workdir,
            identity,
        })
    }

    /// The end and line of a turn that ran on `lease`, which no line shows.
    fn closing(&self, ended: Ended, row: &TurnRow, label: &str, lease: &Secret) -> Closing {
        let hide = |text: &str| lines::redact(text, lease);
        match ended {
            Ended::Answered(Answer::Reply { text, cost_usd }) => Closing {
                cost: cost_usd,
                ..Closing::of(End::Replied, lines::reply(&text))
            },
            Ended::Answered(Answer::Refused) => {
                Closing::of(End::TokenRefused, lines::refused(&row.owner, label))
            }
            Ended::Answered(Answer::Failed(line)) => {
                Closing::of(End::Failed, lines::stopped(&hide(&line)))
            }
            Ended::Answered(Answer::Unavailable(why)) => {
                Closing::of(End::Failed, lines::unavailable(&hide(&why)))
            }
            Ended::Unsafe(why) => {
                (self.log)(&format!("turn {}: {}", row.id, hide(&why)));
                Closing::of(End::Unsafe, lines::UNSAFE.to_owned())
            }
            Ended::Killed => Closing::of(End::Killed, lines::KILLED.to_owned()),
        }
    }

    /// The context of turn `row`, rebuilt from `slack.db`.
    fn context(&self, db: &Db, row: &TurnRow, client: &Client) -> rusqlite::Result<String> {
        let first = db.first_request(&row.channel, &row.thread_ts)?;
        let first_id = first.as_ref().map(|(id, _)| *id);
        let earlier: Vec<Said> = db
            .history(&row.channel, &row.thread_ts, row.id)?
            .into_iter()
            .filter(|(id, _)| Some(*id) != first_id)
            .map(|(_, said)| said)
            .collect();
        let new: Vec<Said> = db
            .turn_items(row.id)?
            .into_iter()
            .map(|(_, said)| said)
            .collect();
        let rows = db.thread_boxes(&row.channel, &row.thread_ts)?;
        let mut call = |request| client.request(request, |_| {});
        let prompts = db.open_prompts(&row.channel, &row.thread_ts)?;
        let boxes = box_lines(&rows, &prompts, &mut call);
        let actions = db.thread_actions(&row.channel, &row.thread_ts)?;
        let rerun = row.rerun_of.map(|lost| db.turn_actions(lost)).transpose()?;
        let parts = Parts {
            first_request: first.as_ref().map(|(_, text)| text.as_str()),
            actions: &actions,
            boxes: &boxes,
            earlier: &earlier,
            new: &new,
            rerun: rerun.as_deref(),
        };
        Ok(context::render(&parts, self.budget))
    }
}

/// What a turn needs before it runs.
struct Ready {
    /// Where the agent reaches the front Proxy.
    proxy: FrontProxy,
    /// The agent's config dir for the starter.
    config_dir: PathBuf,
    /// The thread's workdir.
    workdir: PathBuf,
    /// The starter, as the controller knows them.
    identity: Identity,
}

/// How a turn ended and the line that says so.
struct Closing {
    /// How it ended.
    end: End,
    /// What the thread is shown.
    line: String,
    /// Size of the context it ran with, when it ran.
    bytes: Option<usize>,
    /// Cost the agent reported.
    cost: Option<f64>,
}

impl Closing {
    /// A turn that ended as `end` with `line`, with nothing to log.
    const fn of(end: End, line: String) -> Self {
        Self {
            end,
            line,
            bytes: None,
            cost: None,
        }
    }
}
