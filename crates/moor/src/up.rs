//! `moor up` and `moor down`: set one host up as a controller with a local node.
//!
//! `up` goes from installed binaries to a running controller, an enrolled user and its secrets,
//! a gate that serves `report_step` to the Boxes, and a local node whose machine agent makes
//! Docker Boxes.
//! Every step checks before it acts, so a rerun changes nothing and prints the same lines.

mod files;
mod gate;
mod node;
mod secrets;
mod service;
#[cfg(test)]
mod tests;

use std::io::{BufRead, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use moor_core::request::EnrollReq;
use moor_core::view::user::EnrollValue;
use moor_core::Request;

use crate::cli::GlobalOpts;
use crate::client::{self, ClientError, Target};
use crate::doctor::{answer, line, Check, Status};
use crate::env::Env;

use files::Layout;

/// Pause between looks at the controller socket after the service starts.
const SOCKET_POLL: Duration = Duration::from_millis(100);
/// Looks at the socket before giving up: 30 s in all.
const SOCKET_POLLS: u32 = 300;

/// The command `up` suggests once the host is ready.
const NEXT: &str = "moor start hello --repo OWNER/NAME --prompt \"Say hello and stop\"";

/// Converges this host and prints one line per step; the doctor's report ends it. Returns the
/// exit code: 1 when a step or a doctor check failed.
pub fn up(
    env: &Env,
    opts: &GlobalOpts,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    prompt: &mut dyn Write,
) -> u8 {
    if let Err(failed) = steps(env, input, out, prompt) {
        let _ = out.write_all(line(&failed).as_bytes());
        return 1;
    }
    let report = crate::doctor::run(env, opts, None);
    let _ = out.write_all(report.stdout.as_bytes());
    if report.code == 0 {
        let _ = writeln!(out, "next: {NEXT}");
    }
    report.code
}

/// Each step of `up` in order; the first that fails stops the rest.
fn steps(
    env: &Env,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    prompt: &mut dyn Write,
) -> Result<(), Check> {
    let mut say = |c: &Check| {
        let _ = out.write_all(line(c).as_bytes());
    };
    let layout = Layout::read(env)?;
    std::fs::create_dir_all(&layout.state_dir).map_err(|e| {
        Check::fail(
            "config",
            format!("cannot make {}: {e}", layout.state_dir.display()),
            "",
        )
    })?;
    let (plan, found) = node::plan(env, &layout)?;
    let dir = service::unit_dir(env)
        .ok_or_else(|| Check::fail("service", "HOME is unset", "run moor up as a login user"))?;
    let gate = gate::decide(env, &layout, &dir)?;
    found.iter().for_each(&mut say);
    let key_file = match &gate {
        gate::Gate::Run(plan) => {
            gate::keep_key(plan)?;
            Some(plan.key_file.clone())
        }
        gate::Gate::Left(_) => None,
    };
    say(&files::controller(&layout, gate.runs())?);
    say(&files::user(env, &layout)?);
    let unit = service::Unit::new(env, &layout, plan.egress, key_file)?;
    say(&service::install(&dir, &unit)?);
    if service::lingers(env) == Some(false) {
        say(&Check::warn(
            "service",
            "the controller stops when you log out",
            "run `loginctl enable-linger`",
        ));
    }
    say(&wait_for(&layout)?);
    say(&gate::converge(&dir, &gate)?);
    let target = Target::Socket(layout.socket.clone());
    let ask = |req: Request| -> Result<_, ClientError> {
        client::call(env, &target, &req, |_| {}, |_| {})
    };
    let enrolled = answer::<EnrollValue>(
        &ask,
        Request::Enroll(EnrollReq {
            identity: None,
            email: None,
        }),
    )
    .map_err(|p| Check::fail("user", p.detail, p.fix))?;
    say(&Check::ok(
        "user",
        format!("enrolled as user {}", enrolled.user.get()),
    ));
    for step in secrets::converge(env, &ask, input, prompt) {
        say(&step?);
    }
    node::converge(env, &ask, &layout, &plan, &dir)?
        .iter()
        .for_each(&mut say);
    Ok(())
}

/// Waits until the controller socket takes a connection.
fn wait_for(layout: &Layout) -> Result<Check, Check> {
    for _ in 0..SOCKET_POLLS {
        if UnixStream::connect(&layout.socket).is_ok() {
            return Ok(Check::ok(
                "controller",
                format!("answers at {}", layout.socket.display()),
            ));
        }
        std::thread::sleep(SOCKET_POLL);
    }
    Err(Check::fail(
        "controller",
        format!("no answer at {}", layout.socket.display()),
        format!("read `journalctl --user -u {}`", service::SERVICE),
    ))
}

/// Stops the controller, the gate, the local node's runner and agent, and removes their units; config, state and secrets stay.
pub fn down(env: &Env, out: &mut dyn Write) -> u8 {
    let result = service::unit_dir(env)
        .ok_or_else(|| Check::fail("service", "HOME is unset", ""))
        .and_then(|dir| service::remove(&dir));
    let check = match result {
        Ok(true) => Check::ok(
            "service",
            "stopped and removed; config, state and secrets kept",
        ),
        Ok(false) => Check::ok("service", "not installed"),
        Err(c) => c,
    };
    let _ = out.write_all(line(&check).as_bytes());
    u8::from(check.status == Status::Fail)
}
