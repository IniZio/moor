//! Client side of controller mode: send the request to the daemon, stream its
//! progress, and run interactive commands locally.

use std::io::Write;

use moor_core::{Request, Response};

use super::Target;
use crate::cli::GlobalOpts;
use crate::env::Env;

/// Sends `request` to the controller at `target` and returns its answer.
///
/// Text mode prints progress frames to stderr and a `run` command's output to stdout as they
/// arrive. `--json` drops progress and prints the command's output to stderr, so stdout stays
/// one envelope.
#[must_use]
pub fn run(env: &Env, target: &Target, request: &Request, opts: &GlobalOpts) -> Response {
    let progress = |msg: &str| {
        if !opts.json {
            let mut err = std::io::stderr();
            let _ = writeln!(err, "moor: {msg}");
            let _ = err.flush();
        }
    };
    let output = |data: &[u8]| {
        let mut sink: Box<dyn Write> = if opts.json {
            Box::new(std::io::stderr())
        } else {
            Box::new(std::io::stdout())
        };
        let _ = sink.write_all(data);
        let _ = sink.flush();
    };
    let result = super::call(env, target, request, progress, output);
    match result {
        Ok(resp) => resp,
        Err(e) => super::failure_response(request.cmd(), &e),
    }
}

#[cfg(test)]
mod tests;
