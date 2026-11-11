//! `moor forward`: a port of a Box on this computer's loopback, until Ctrl-C.

use std::io::Write;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use moor_client::port_forward::{self, Event, Say, Spec};
use moor_client::Client;

use super::{configured_target, Target};
use crate::env::Env;

/// Forwards `spec` through the controller `env` names until SIGINT or SIGTERM, printing each
/// event to stdout; the exit code, 0 after a forward that ended by signal.
#[must_use]
pub fn run(env: &Env, spec: &Spec) -> ExitCode {
    let fail = |why: &str| {
        let _ = writeln!(std::io::stderr(), "moor: {why}");
        ExitCode::from(1)
    };
    let client = match configured_target(env) {
        Some(Target::Socket(path)) => Client::new(path),
        Some(Target::Ssh(_)) => return fail("moor forward needs a controller socket, not ssh"),
        None => return fail("moor forward needs a controller: set MOOR_CONTROLLER"),
    };
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        if let Err(e) = signal_hook::flag::register(signal, Arc::clone(&stop)) {
            return fail(&format!("cannot watch for signals: {e}"));
        }
    }
    let shown = spec.clone();
    let say: Say = Arc::new(move |event: Event| {
        let _ = writeln!(std::io::stdout(), "{}", event.describe(&shown));
    });
    match port_forward::run(&client, spec, &stop, say) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => fail(&e.to_string()),
    }
}
