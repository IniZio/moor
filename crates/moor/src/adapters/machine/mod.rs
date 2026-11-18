//! Machines: the Driver over machines whose agents provide Boxes themselves.
//!
//! A machine's agent makes its Boxes however it likes and exposes only the Box protocol
//! (`moor_client::link::boxes`), which the runner holding its link splices to the controller
//! without reading. This driver knows no Docker, no ssh and no image: each call is a Box request
//! on one stream, and everything past open, sleep, wake, close, list and exec (files, batches,
//! detached runs) is composed over exec in [`over_exec`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_client::link::boxes::{BoxAnswer, BoxOp, ReportState};
use moor_client::link::reach::Runner;
use moor_core::box_key::{parse_machine_handle, parse_pool_slug, BoxKey, ControllerId};
use moor_core::name::{BoxHandle, BoxPath, ProviderId};
use moor_core::seam::driver::{
    AcquireReq, Acquired, BoxListing, BoxStep, Cancelled, Driver, DriverCaps, DriverError,
    ExecOutcome, ExecSpec, Followed, Forwards, HandlesListing, Launched, Place, Released, RunEnd,
    SleepOutcome, StepDone,
};
use moor_core::seam::proc::{Deadline, Pipes};

pub use env::SentEnvs;
use stream::{ask, blocking, failed, open, unreached};

mod env;
mod exec;
mod over_exec;
mod relink;
mod stream;
mod term;
pub use term::box_term;

#[cfg(test)]
pub(crate) use relink::link_down;
use relink::relinked;

/// How the controller reaches the runner that holds a machine's link.
pub trait Reach: Send + Sync {
    /// The runner holding machine `machine_id`'s link now.
    ///
    /// # Errors
    /// No runner holds it, or one across the network and no key.
    fn runner(&self, machine_id: i64) -> Result<Runner, String>;
}

/// The Driver over machines that provide Boxes themselves.
pub struct Machines {
    /// Where each machine's runner is.
    reach: Arc<dyn Reach>,
    /// This moor binary, which `box-term` runs as.
    moor: PathBuf,
    /// What [`Driver::set_env`] last sent each Box, shared by every request of a controller.
    env_sent: Arc<SentEnvs>,
}

impl Machines {
    /// The driver reaching machines through `reach`, whose terminals run `moor box-term` of the
    /// binary at `moor`.
    #[must_use]
    pub fn new(reach: Arc<dyn Reach>, moor: PathBuf) -> Self {
        Self {
            reach,
            moor,
            env_sent: Arc::default(),
        }
    }

    /// The machine and Box slug `handle` names.
    fn seat(handle: &BoxHandle) -> Result<(i64, &str), DriverError> {
        parse_machine_handle(handle.as_str())
            .ok_or_else(|| failed(format!("{} is no Box of a machine", handle.as_str())))
    }

    /// The runner holding `machine`'s link.
    fn runner(&self, machine: i64) -> Result<Runner, DriverError> {
        self.reach.runner(machine).map_err(unreached)
    }

    /// Sends `op` to `machine`'s agent and reads its answer, handing each note to `note`.
    pub(super) fn call(
        &self,
        machine: i64,
        op: &BoxOp,
        note: &mut (dyn FnMut(&str) + Send),
    ) -> Result<BoxAnswer, DriverError> {
        let runner = self.runner(machine)?;
        blocking(async {
            let mut conn = open(&runner, machine).await?;
            ask(&mut conn, op, note).await
        })
    }

    /// Sends the verb for `handle` that `op` builds from the slug and wants `Done` back.
    fn done(&self, handle: &BoxHandle, verb: impl Fn(String) -> BoxOp) -> Result<(), DriverError> {
        let (machine, slug) = Self::seat(handle)?;
        match self.call_again(machine, &verb(slug.to_owned()), &mut |_| {})? {
            BoxAnswer::Done => Ok(()),
            BoxAnswer::Gone => Err(DriverError::NotFound),
            other => Err(unexpected(&other)),
        }
    }

    /// The open request for `req`: its seat's machine, the slug and the handle.
    fn planned(req: &AcquireReq) -> Result<(i64, BoxOp, BoxHandle), DriverError> {
        let seat = req.pool_machine.as_ref().ok_or_else(|| {
            failed(format!(
                "provider {} names no machine for the Box",
                req.provider.as_str()
            ))
        })?;
        let key = BoxKey::new(req.box_id);
        let op = BoxOp::Open {
            slug: key.pool_slug(&seat.controller, seat.inc),
            class: req.class.clone(),
            cache: None,
        };
        Ok((seat.machine, op, BoxHandle::new(key.machine_handle(seat))))
    }
}

/// The error for an answer a verb does not give.
fn unexpected(answer: &BoxAnswer) -> DriverError {
    failed(format!("the machine answered {answer:?}"))
}

impl Forwards for Machines {}

impl Driver for Machines {
    fn caps(&self, _provider: &ProviderId) -> Result<DriverCaps, DriverError> {
        Ok(DriverCaps {
            has_workdir: true,
            cache_volumes: false,
        })
    }

    fn acquire(&self, req: &AcquireReq) -> Result<Acquired, DriverError> {
        self.acquire_noting(req, &mut |_| {})
    }

    fn acquire_noting(
        &self,
        req: &AcquireReq,
        note: &mut dyn FnMut(&str),
    ) -> Result<Acquired, DriverError> {
        let (machine, op, handle) = Self::planned(req)?;
        env::forget_opened(self, &handle);
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let answer = std::thread::scope(|s| {
            // A lost link is the caller's to wait out, as the open outlives it.
            let call = s.spawn(move || {
                self.call(machine, &op, &mut |line| {
                    let _ = tx.send(line.to_owned());
                })
            });
            for line in rx {
                note(&line);
            }
            call.join()
                .unwrap_or_else(|_| Err(failed("the Box open panicked".to_owned())))
        })?;
        match answer {
            BoxAnswer::Opened { workdir, .. } => Ok(Acquired {
                handle,
                workdir: BoxPath::restore(&workdir)
                    .map_err(|e| failed(format!("the machine named a bad workdir: {e}")))?,
            }),
            other => Err(unexpected(&other)),
        }
    }

    /// The agent makes the Box on a spawn of its own, so an open whose stream is lost goes on; a
    /// second open of the slug waits for it under the Box's flight and finds the same Box.
    fn acquire_outlives_caller(&self, handle: &BoxHandle) -> bool {
        parse_machine_handle(handle.as_str()).is_some()
    }

    fn planned_handle(&self, req: &AcquireReq) -> BoxHandle {
        Self::planned(req).map_or_else(|_| BoxHandle::new(""), |(_, _, handle)| handle)
    }

    fn release(&self, handle: &BoxHandle) -> Result<Released, DriverError> {
        let (machine, slug) = Self::seat(handle)?;
        let op = BoxOp::Close {
            slug: slug.to_owned(),
        };
        let answer = self.call_again(machine, &op, &mut |_| {});
        env::forget_closed(self, slug, &answer);
        match answer? {
            BoxAnswer::Done => Ok(Released::Deleted),
            BoxAnswer::Gone => Ok(Released::AlreadyGone),
            other => Err(unexpected(&other)),
        }
    }

    fn list(&self, _provider: &ProviderId) -> Result<Vec<BoxListing>, DriverError> {
        // Nothing here names a machine from a provider; `list_handles` asks the holders.
        Err(DriverError::Unsupported("listing a provider's Boxes"))
    }

    fn list_handles(&self, handles: &[BoxHandle]) -> Result<HandlesListing, DriverError> {
        let mut asked: Vec<(i64, ControllerId, Vec<&BoxHandle>)> = Vec::new();
        for handle in handles {
            let Some((machine, slug)) = parse_machine_handle(handle.as_str()) else {
                continue;
            };
            let Some((controller, ..)) = parse_pool_slug(slug) else {
                continue;
            };
            match asked
                .iter_mut()
                .find(|(m, c, _)| *m == machine && *c == controller)
            {
                Some((.., group)) => group.push(handle),
                None => asked.push((machine, controller, vec![handle])),
            }
        }
        let mut listing = HandlesListing::default();
        for (machine, controller, group) in asked {
            let op = BoxOp::List { controller };
            // A machine that cannot be reached leaves its handles unknown, never missing.
            let Ok(BoxAnswer::Listed { boxes }) = self.call(machine, &op, &mut |_| {}) else {
                continue;
            };
            for handle in group {
                listing.answered.push(handle.clone());
                let held = parse_machine_handle(handle.as_str())
                    .and_then(|(_, slug)| boxes.iter().find(|b| b.slug == slug));
                if let Some(held) = held {
                    listing.listed.push(BoxListing {
                        handle: handle.clone(),
                        provider: None,
                        asleep: held.state == ReportState::Asleep,
                    });
                }
            }
        }
        Ok(listing)
    }

    fn workdir(&self, handle: &BoxHandle) -> Result<BoxPath, DriverError> {
        let spec = ExecSpec::new(Place::Agent, vec!["pwd".to_owned()]);
        let (out, outcome) = relinked(|| over_exec::capture_out(self, handle, &spec))?;
        match outcome {
            ExecOutcome::Exited { code: 0 } => BoxPath::restore(out.trim())
                .map_err(|e| failed(format!("the Box's workdir is unreadable: {e}"))),
            _ => Err(failed("the Box's workdir could not be read".to_owned())),
        }
    }

    fn exec(
        &self,
        handle: &BoxHandle,
        spec: &ExecSpec,
        deadline: Deadline,
    ) -> Result<ExecOutcome, DriverError> {
        if spec.repeatable {
            return relinked(|| self.run_captured(handle, spec, deadline));
        }
        self.run_captured(handle, spec, deadline)
    }

    fn exec_piped(
        &self,
        handle: &BoxHandle,
        spec: &ExecSpec,
        pipes: Pipes<'_>,
    ) -> Result<ExecOutcome, DriverError> {
        self.run_piped(handle, spec, pipes)
    }

    fn copy_out(
        &self,
        handle: &BoxHandle,
        place: Place,
        from: &BoxPath,
        to: &Path,
    ) -> Result<(), DriverError> {
        over_exec::copy::copy_out(self, handle, place, from, to)
    }

    fn peek_file(
        &self,
        handle: &BoxHandle,
        place: Place,
        path: &BoxPath,
    ) -> Result<Option<String>, DriverError> {
        over_exec::peek_file(self, handle, place, path)
    }

    fn put_file(
        &self,
        handle: &BoxHandle,
        place: Place,
        path: &BoxPath,
        bytes: &[u8],
        mode: u32,
    ) -> Result<(), DriverError> {
        over_exec::put_file(self, handle, place, path, (bytes, mode))
    }

    fn connect_argv(&self, handle: &BoxHandle, place: Place) -> Result<Vec<String>, DriverError> {
        let (machine, slug) = Self::seat(handle)?;
        let addr = match self.runner(machine)? {
            Runner::Local(socket) => socket.to_string_lossy().into_owned(),
            Runner::Remote { addr, .. } => addr,
        };
        Ok([
            &*self.moor.to_string_lossy(),
            "box-term",
            "--runner",
            &addr,
            &machine.to_string(),
            slug,
            "--place",
            place.as_str(),
            "--",
        ]
        .map(str::to_owned)
        .to_vec())
    }

    fn batch(
        &self,
        handle: &BoxHandle,
        _connect: &[String],
        place: Place,
        steps: &[BoxStep],
        deadline: Deadline,
    ) -> Vec<Result<StepDone, DriverError>> {
        over_exec::batch(self, handle, place, steps, deadline)
    }

    fn launch_run(
        &self,
        handle: &BoxHandle,
        run: i64,
        spec: &ExecSpec,
    ) -> Result<Launched, DriverError> {
        over_exec::launch_run(self, handle, run, spec)
    }

    fn follow_run(
        &self,
        handle: &BoxHandle,
        run: i64,
        from: u64,
        out: &mut dyn std::io::Write,
        deadline: Deadline,
    ) -> Result<Followed, DriverError> {
        over_exec::follow_run(self, handle, run, from, out, deadline)
    }

    fn end_run(
        &self,
        handle: &BoxHandle,
        run: i64,
        workdir: Option<&BoxPath>,
    ) -> Result<RunEnd, DriverError> {
        over_exec::end_run(self, handle, run, workdir)
    }

    fn cancel_run(
        &self,
        handle: &BoxHandle,
        run: i64,
        grace_secs: u32,
    ) -> Result<Cancelled, DriverError> {
        over_exec::cancel_run(self, handle, run, grace_secs)
    }

    fn set_env(
        &self,
        handle: &BoxHandle,
        vars: &[(String, String)],
        ca_pem: Option<&str>,
    ) -> Result<(), DriverError> {
        env::send(self, handle, vars, ca_pem)
    }

    fn sleep(&self, handle: &BoxHandle) -> Result<SleepOutcome, DriverError> {
        self.done(handle, |slug| BoxOp::Sleep { slug })
            .map(|()| SleepOutcome::Slept)
    }

    fn wake(&self, handle: &BoxHandle) -> Result<(), DriverError> {
        let (machine, slug) = Self::seat(handle)?;
        let op = BoxOp::Wake {
            slug: slug.to_owned(),
        };
        match self.call_again(machine, &op, &mut |_| {})? {
            BoxAnswer::Woke { .. } => Ok(()),
            BoxAnswer::Gone => Err(DriverError::NotFound),
            other => Err(unexpected(&other)),
        }
    }
}
