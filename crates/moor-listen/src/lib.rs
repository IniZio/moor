//! Holds a `subscriptions/listen` stream open on the moor gate and says one line per Box event
//! worth waking for. It knows nothing of whoever reads those lines.

mod gate;
mod idle;
mod sse;
mod token;
mod watch;

use std::io;
use std::time::Duration;

pub use token::{Bearer, Fail, Session, Shell};

/// How a listen runs.
pub struct Listen {
    /// The gate's MCP endpoint, such as `https://host/mcp`.
    pub url: String,
    /// Where tokens come from.
    pub bearer: Box<dyn Bearer>,
    /// Silence on the stream after which it counts as dead.
    pub idle: Duration,
    /// How long to wait before looking again for a sign-in.
    pub signed_out: Duration,
    /// Names of the only Boxes to watch; empty watches every Box the login can see.
    pub only: Vec<String>,
}

impl Listen {
    /// Silence after which a stream counts as dead, unless told otherwise.
    pub const IDLE: Duration = Duration::from_secs(45);

    /// How long to wait for a sign-in, unless told otherwise.
    pub const SIGNED_OUT: Duration = Duration::from_secs(60);

    /// Runs until `say` fails, which is when nobody reads the lines any more, or, when `only`
    /// names Boxes, until all of them have closed.
    ///
    /// # Errors
    /// `say` failed.
    pub fn run(self, say: &mut dyn FnMut(&str) -> io::Result<()>) -> io::Result<()> {
        let gate = gate::Gate::new(self.url, self.bearer, self.idle);
        watch::run(&gate, self.signed_out, self.only, say)
    }
}
