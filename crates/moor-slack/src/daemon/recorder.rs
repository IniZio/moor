//! The Socket Mode thread's intake.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;

use moor_core::secret::Secret;

use super::stop::sleep_until_stopped;
use crate::api::Api;
use crate::db::Db;
use crate::email::Emails;
use crate::identity::Link;
use crate::socket::{self, AfterAck, Inbound, Intake, Kept, NotKept};
use crate::worker::{self, Job};
use crate::{login, mcp, slash, Log};

/// The Socket Mode thread's intake: records each mention and command in `slack.db`, then
/// queues its job, all before Slack gets the ack.
pub(super) struct Recorder {
    /// The Socket Mode thread's own `slack.db` connection.
    pub(super) db: Db,
    /// Web API client, for the login modal.
    pub(super) api: Arc<Api>,
    /// Bot token for opening the login modal.
    pub(super) bot_token: Arc<Secret>,
    /// The controller, which the add-server modal asks as the Slack user before its ack.
    pub(super) link: Link,
    /// The profile email each of those asks carries when it enrolls.
    pub(super) emails: Emails,
    /// The worker's queue.
    pub(super) jobs: Sender<Job>,
    /// Daemon log.
    pub(super) log: Log,
    /// The current time in unix seconds.
    pub(super) clock: fn() -> i64,
}

impl Recorder {
    /// Runs Socket Mode on its own thread, feeding this intake, until `stop` is set.
    pub(super) fn spawn(
        self,
        app_token: Secret,
        roleplay: Option<socket::Roleplay>,
        stop: &Arc<AtomicBool>,
    ) -> thread::JoinHandle<()> {
        let stop = Arc::clone(stop);
        thread::spawn(move || {
            let sleep = |d| sleep_until_stopped(&stop, d);
            let api = Arc::clone(&self.api);
            socket::run(&api, &app_token, &self, roleplay.as_ref(), &stop, &sleep);
        })
    }
}

impl Intake for Recorder {
    /// Keeps a mention, a button press on a prompt, a command or a login submission.
    fn take(&self, item: Inbound) -> Result<Kept, NotKept> {
        let now = (self.clock)();
        let kept = match item {
            Inbound::Mention(mention) => {
                worker::record(&mention, &self.db, now).map(|job| (Kept::default(), job))
            }
            Inbound::Slash(slash) => slash::record(&slash, &self.db, now),
            Inbound::Submission(sub) if mcp::owns(&sub.account) => {
                let mut call = |user: &str, req| self.emails.call(&self.link, &self.db, user, req);
                let ack = mcp::submission(&sub, &mut call);
                Ok((
                    Kept {
                        ack: Some(ack),
                        then: None,
                    },
                    None,
                ))
            }
            Inbound::Submission(sub) => login::submission(sub, &self.db, now).map(|(ack, job)| {
                (
                    Kept {
                        ack: Some(ack),
                        then: None,
                    },
                    job,
                )
            }),
            Inbound::Click(click) => {
                worker::click(&click, &self.db, now).map(|job| (Kept::default(), job))
            }
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
        let (trigger_id, form) = match then {
            AfterAck::Login { trigger_id, form } => (trigger_id, login::form(form)),
            AfterAck::Mcp { trigger_id } => (trigger_id, mcp::form()),
        };
        if let Err(e) = self.api.open_view(&self.bot_token, &trigger_id, &form) {
            (self.log)(&format!("slack: modal failed: {e}"));
        }
    }
}
