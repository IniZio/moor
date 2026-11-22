//! `moor up` and `moor down`: set one host up as a controller with a local node.
//!
//! `up` goes from installed binaries to a running controller, an enrolled user and its secrets,
//! a gate that serves `report_step` to the Boxes, and a local node whose machine agent makes
//! Docker Boxes.
//! Every step checks before it acts, so a rerun changes nothing and prints the same lines.

mod files;
mod gate;
mod node;
mod relay;
mod secrets;
pub(crate) mod service;
#[cfg(test)]
mod tests;

use std::io::{BufRead, Write};
use std::path::Path;
use std::time::Duration;

use moor_core::request::{EnrollReq, ShowReq};
use moor_core::seam::clock::Clock as _;
use moor_core::view::user::EnrollValue;
use moor_core::Request;

use crate::adapters::clock::SystemClock;
use crate::cli::GlobalOpts;
use crate::client::{self, ClientError, Target};
use crate::doctor::{answer, line, Ask, Check, Status};
use crate::env::Env;

use files::Layout;

/// Pause between looks at the controller socket after the service starts.
const SOCKET_POLL: Duration = Duration::from_millis(100);
/// How long one hello waits for its reply.
const HELLO_WAIT: Duration = Duration::from_secs(5);
/// How long `down` waits for a controller's hello before it takes the controller for gone.
const DOWN_HELLO_WAIT: Duration = Duration::from_secs(2);
/// How long the controller has to answer after the service starts.
const SOCKET_WAIT: Duration = Duration::from_secs(30);

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
    let report = crate::doctor::run_with(env, opts, None, false);
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
    let (plan, relay, found) = node::plan(env, &layout)?;
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
    say(&service::install(&dir, &unit, &layout, gate.fronts())?);
    if service::lingers(env) == Some(false) {
        say(&Check::warn(
            "service",
            "the controller stops when you log out",
            "run `loginctl enable-linger`",
        ));
    }
    say(&wait_for(&layout.socket, SOCKET_WAIT)?);
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
    node::converge(env, &ask, &layout, &plan, &relay, &dir)?
        .iter()
        .for_each(&mut say);
    Ok(())
}

/// Waits up to `wait` until the controller answers a hello on its socket. systemd holds the
/// socket, so a connection alone succeeds even while the service fails to start.
fn wait_for(socket: &Path, wait: Duration) -> Result<Check, Check> {
    let clock = SystemClock::real();
    let deadline = clock.instant() + wait;
    let mut last = String::from("nothing tried");
    loop {
        let left = deadline.saturating_duration_since(clock.instant());
        if left.is_zero() {
            break;
        }
        match client::hello(socket, left.min(HELLO_WAIT)) {
            Ok(()) => {
                return Ok(Check::ok(
                    "controller",
                    format!("answers at {}", socket.display()),
                ))
            }
            Err(e) => last = e,
        }
        clock.sleep(SOCKET_POLL);
    }
    Err(Check::fail(
        "controller",
        format!("no answer at {}: {last}", socket.display()),
        format!("read `journalctl --user -u {}`", service::SERVICE),
    ))
}

/// The names of the Boxes open and running; none when no controller answers.
fn open_boxes(ask: Ask<'_>) -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct Open {
        name: String,
        lifecycle: String,
    }
    #[derive(serde::Deserialize)]
    struct Listed {
        boxes: Option<Vec<Open>>,
    }
    let req = Request::Show(ShowReq {
        target: None,
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
        transcript: false,
        everyone: false,
    });
    answer::<Listed>(ask, req)
        .ok()
        .and_then(|l| l.boxes)
        .unwrap_or_default()
        .into_iter()
        .filter(|b| matches!(b.lifecycle.as_str(), "ready" | "setting_up"))
        .map(|b| b.name)
        .collect()
}

/// The failure `down` ends with when `boxes` are open and running.
fn refusal(boxes: &[String]) -> Option<Check> {
    if boxes.is_empty() {
        return None;
    }
    Some(Check::fail(
        "boxes",
        format!(
            "{} open and running: {}; stopping the services leaves their containers with no controller or agent",
            if boxes.len() == 1 { "1 Box is" } else { "Boxes are" },
            boxes.join(", ")
        ),
        "close each with `moor close <name>`, or run `moor down --force` to stop anyway",
    ))
}

/// Stops and removes every unit `up` wrote; config, state and secrets stay. Refuses while Boxes run, unless `force`.
pub fn down(env: &Env, force: bool, out: &mut dyn Write) -> u8 {
    down_with(env, force, &|| running_boxes(env), out)
}

/// The Boxes of the controller on this host, asked only when it answers a hello, so a controller
/// that is broken, which is when `down` is wanted most, does not hold `down` up.
fn running_boxes(env: &Env) -> Vec<String> {
    let Some(Target::Socket(socket)) = client::configured_target(env) else {
        return Vec::new();
    };
    if client::hello(&socket, DOWN_HELLO_WAIT).is_err() {
        return Vec::new();
    }
    let target = Target::Socket(socket);
    open_boxes(&|req: Request| client::call(env, &target, &req, |_| {}, |_| {}))
}

/// [`down`], where `running` names the Boxes that run.
fn down_with(env: &Env, force: bool, running: &dyn Fn() -> Vec<String>, out: &mut dyn Write) -> u8 {
    if !force {
        if let Some(refused) = refusal(&running()) {
            let _ = out.write_all(line(&refused).as_bytes());
            return 1;
        }
    }
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
