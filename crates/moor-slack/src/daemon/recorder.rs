//! The Socket Mode thread's intake.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use moor_core::secret::Secret;

use crate::api::Api;
use crate::command::Allow;
use crate::db::Db;
use crate::socket::{AfterAck, Inbound, Intake, Kept, NotKept};
use crate::worker::{self, Job};
use crate::{login, slash, Log};

/// The Socket Mode thread's intake: records each mention and command in `slack.db`, then
/// queues its job, all before Slack gets the ack.
pub(super) struct Recorder {
    /// The Socket Mode thread's own `slack.db` connection.
    pub(super) db: Db,
    /// Channel ids where mentions are answered.
    pub(super) channels: Vec<String>,
    /// User ids who may start Boxes.
    pub(super) users: Vec<String>,
    /// The front's state dir, which holds the token files.
    pub(super) state_dir: PathBuf,
    /// Web API client, for the login modal.
    pub(super) api: Arc<Api>,
    /// Bot token for opening the login modal.
    pub(super) bot_token: Arc<Secret>,
    /// The worker's queue.
    pub(super) jobs: Sender<Job>,
    /// Daemon log.
    pub(super) log: Log,
    /// The current time in unix seconds.
    pub(super) clock: fn() -> i64,
}

impl Intake for Recorder {
    /// Keeps a mention, a button press on a prompt, a command or a login submission.
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
        let AfterAck::OpenLogin { trigger_id, agent } = then;
        let form = login::form(agent);
        if let Err(e) = self.api.open_view(&self.bot_token, &trigger_id, &form) {
            (self.log)(&format!("slack: login modal failed: {e}"));
        }
    }
}
