//! moor-notify: a front that posts the controller's webhook deliveries, so a client that
//! started Boxes and walked away learns when one idles or fails without polling.
//!
//! It decides nothing: the controller queues each delivery with its body built, and this
//! front claims them, signs and posts each, and reports how it ended.

use std::io::{self, Write as _};
use std::sync::Arc;

pub mod deliver;
pub mod sign;
pub mod work;

/// Where the front writes its log lines.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// A log that writes `moor-notify: <line>` to stderr.
#[must_use]
pub fn stderr_log() -> Log {
    Arc::new(|msg: &str| {
        let _ = writeln!(io::stderr(), "moor-notify: {msg}");
    })
}
