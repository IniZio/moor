//! A second runner on a rig's controller, as another replica would be.

use super::{spawn, Rig, Runner};

impl Rig {
    /// A second runner on the rig's controller, started with `args`, as another replica would
    /// be: it holds no link until an agent dials its port.
    pub fn replica(&self, args: &[&str]) -> Replica {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        let (runner, _, agents, links) = spawn(self.dir(), &self.dir().join("replica"), &args);
        Replica {
            _runner: runner,
            agents,
            links,
        }
    }
}

/// A rig's second runner, killed when dropped.
pub struct Replica {
    _runner: Runner,
    /// The port it takes agent links on.
    pub agents: u16,
    /// The port it takes other runners' and the controller's dials on, when started with
    /// `--links`.
    pub links: Option<u16>,
}
