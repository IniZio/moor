//! One watch per Slack user: the readers that follow each user's Boxes on the controller.

use std::collections::HashSet;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;

use crate::dial::{read_events, Tagged};
use crate::identity::Link;
use crate::Log;

/// The users whose watch is open, and what opens another.
pub(crate) struct Watchers {
    /// The controller and team.
    link: Link,
    /// Where each reader forwards what it sees.
    feed: Sender<Tagged>,
    /// Daemon log.
    log: Log,
    /// The Slack users a reader follows; a reader is never stopped.
    started: HashSet<String>,
}

impl Watchers {
    /// A supervisor that starts readers forwarding to `feed`.
    pub(crate) fn new(link: Link, feed: Sender<Tagged>, log: Log) -> Self {
        Self {
            link,
            feed,
            log,
            started: HashSet::new(),
        }
    }

    /// Starts a reader for each of `users` that has none. A user with no valid identity gets
    /// none, and the log says so once.
    pub(crate) fn follow(&mut self, users: impl IntoIterator<Item = String>) {
        for user in users {
            if self.started.contains(&user) {
                continue;
            }
            match self.link.identity(&user) {
                Ok(identity) => {
                    let client = self.link.client_as(identity.clone());
                    let (feed, log) = (self.feed.clone(), Arc::clone(&self.log));
                    thread::spawn(move || {
                        read_events(&client, &identity, &feed, &*log, &thread::sleep);
                    });
                }
                Err(why) => (self.log)(&format!("controller: no watch for {user}: {why}")),
            }
            self.started.insert(user);
        }
    }
}
