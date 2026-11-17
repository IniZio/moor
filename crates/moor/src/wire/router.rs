//! The one Driver the controller holds: a node whose machine provides Boxes itself goes to
//! [`Machines`], everything else stays on [`Crabbox`].
//!
//! A Box is routed by where it lives. A call that names a provider goes by the provider: the
//! runtime of a machine that provides Boxes itself is [`BOXES_HOST`]. A call that names a Box goes
//! by its handle, which `Machines` spells `m<machine>/<slug>` and no crabbox handle does. Every
//! method is forwarded, the defaulted ones too, so no trait default answers for a driver that
//! overrides it.

use std::path::Path;

use moor_core::box_key::parse_machine_handle;
use moor_core::config::BOXES_HOST;
use moor_core::name::{BoxHandle, BoxPath, ProviderId};
use moor_core::seam::driver::{
    AcquireReq, Acquired, BoxListing, BoxStep, Cancelled, Driver, DriverCaps, DriverError,
    ExecOutcome, ExecSpec, Followed, Forwards, HandlesListing, Launched, LocalTunnel, Place,
    Released, RunEnd, SleepOutcome, StepDone, TunnelHandle,
};
use moor_core::seam::proc::{Deadline, Pipes};

use crate::adapters::crabbox::Crabbox;
use crate::adapters::machine::Machines;

/// Crabbox for the nodes it serves, and [`Machines`] for the machines that provide Boxes.
pub(super) struct Routed {
    /// The Boxes on crabbox providers.
    pub(super) crabbox: Crabbox,
    /// The Boxes machines provide themselves.
    pub(super) machines: Machines,
}

impl Routed {
    /// The driver for a Box on `provider`.
    fn of_provider(&self, provider: &ProviderId) -> &dyn Driver {
        if provider.base() == BOXES_HOST {
            &self.machines
        } else {
            &self.crabbox
        }
    }

    /// The driver for the Box `handle` names.
    fn of(&self, handle: &BoxHandle) -> &dyn Driver {
        if parse_machine_handle(handle.as_str()).is_some() {
            &self.machines
        } else {
            &self.crabbox
        }
    }
}

impl Forwards for Routed {
    fn port_listens(&self, handle: &BoxHandle, port: u16) -> Result<bool, DriverError> {
        self.of(handle).port_listens(handle, port)
    }

    fn forward_piped(
        &self,
        handle: &BoxHandle,
        port: u16,
        pipes: Pipes<'_>,
    ) -> Result<(), DriverError> {
        self.of(handle).forward_piped(handle, port, pipes)
    }
}

impl Driver for Routed {
    fn caps(&self, provider: &ProviderId) -> Result<DriverCaps, DriverError> {
        self.of_provider(provider).caps(provider)
    }

    fn acquire(&self, req: &AcquireReq) -> Result<Acquired, DriverError> {
        self.of_provider(&req.provider).acquire(req)
    }

    fn acquire_noting(
        &self,
        req: &AcquireReq,
        note: &mut dyn FnMut(&str),
    ) -> Result<Acquired, DriverError> {
        self.of_provider(&req.provider).acquire_noting(req, note)
    }

    fn planned_handle(&self, req: &AcquireReq) -> BoxHandle {
        self.of_provider(&req.provider).planned_handle(req)
    }

    fn acquire_outlives_caller(&self, handle: &BoxHandle) -> bool {
        self.of(handle).acquire_outlives_caller(handle)
    }

    fn cancel_acquire(&self, handle: &BoxHandle) -> Result<(), DriverError> {
        self.of(handle).cancel_acquire(handle)
    }

    fn adopt_noting(
        &self,
        req: &AcquireReq,
        note: &mut dyn FnMut(&str),
    ) -> Result<Acquired, DriverError> {
        self.of_provider(&req.provider).adopt_noting(req, note)
    }

    fn release(&self, handle: &BoxHandle) -> Result<Released, DriverError> {
        self.of(handle).release(handle)
    }

    fn manual_release(&self, handle: &BoxHandle) -> Option<String> {
        self.of(handle).manual_release(handle)
    }

    fn list(&self, provider: &ProviderId) -> Result<Vec<BoxListing>, DriverError> {
        self.of_provider(provider).list(provider)
    }

    fn list_handles(&self, handles: &[BoxHandle]) -> Result<HandlesListing, DriverError> {
        let (ours, theirs): (Vec<BoxHandle>, Vec<BoxHandle>) = handles
            .iter()
            .cloned()
            .partition(|h| parse_machine_handle(h.as_str()).is_some());
        let mut listing = if theirs.is_empty() && !handles.is_empty() {
            HandlesListing::default()
        } else {
            self.crabbox.list_handles(&theirs)?
        };
        if !ours.is_empty() {
            let more = self.machines.list_handles(&ours)?;
            listing.answered.extend(more.answered);
            listing.listed.extend(more.listed);
            listing.said.extend(more.said);
        }
        Ok(listing)
    }

    fn workdir(&self, handle: &BoxHandle) -> Result<BoxPath, DriverError> {
        self.of(handle).workdir(handle)
    }

    fn exec(
        &self,
        handle: &BoxHandle,
        spec: &ExecSpec,
        deadline: Deadline,
    ) -> Result<ExecOutcome, DriverError> {
        self.of(handle).exec(handle, spec, deadline)
    }

    fn exec_piped(
        &self,
        handle: &BoxHandle,
        spec: &ExecSpec,
        pipes: Pipes<'_>,
    ) -> Result<ExecOutcome, DriverError> {
        self.of(handle).exec_piped(handle, spec, pipes)
    }

    fn copy_out(
        &self,
        handle: &BoxHandle,
        place: Place,
        from: &BoxPath,
        to: &Path,
    ) -> Result<(), DriverError> {
        self.of(handle).copy_out(handle, place, from, to)
    }

    fn stream_files(
        &self,
        handle: &BoxHandle,
        dir: &BoxPath,
        names: &[String],
        deadline: Deadline,
        sink: &mut dyn FnMut(&str, Option<&mut dyn std::io::Read>),
    ) -> Result<(), DriverError> {
        self.of(handle)
            .stream_files(handle, dir, names, deadline, sink)
    }

    fn peek_file(
        &self,
        handle: &BoxHandle,
        place: Place,
        path: &BoxPath,
    ) -> Result<Option<String>, DriverError> {
        self.of(handle).peek_file(handle, place, path)
    }

    fn put_file(
        &self,
        handle: &BoxHandle,
        place: Place,
        path: &BoxPath,
        bytes: &[u8],
        mode: u32,
    ) -> Result<(), DriverError> {
        self.of(handle).put_file(handle, place, path, bytes, mode)
    }

    fn connect_argv(&self, handle: &BoxHandle, place: Place) -> Result<Vec<String>, DriverError> {
        self.of(handle).connect_argv(handle, place)
    }

    fn reverse_tunnel(
        &self,
        handle: &BoxHandle,
        box_port: u16,
        socket: &Path,
    ) -> Result<Box<dyn TunnelHandle>, DriverError> {
        self.of(handle).reverse_tunnel(handle, box_port, socket)
    }

    fn refresh_login(&self, handle: &BoxHandle) -> Result<(), DriverError> {
        self.of(handle).refresh_login(handle)
    }

    fn set_env(
        &self,
        handle: &BoxHandle,
        vars: &[(String, String)],
        ca_pem: Option<&str>,
    ) -> Result<(), DriverError> {
        self.of(handle).set_env(handle, vars, ca_pem)
    }

    fn local_tunnel(&self, handle: &BoxHandle, box_port: u16) -> Result<LocalTunnel, DriverError> {
        self.of(handle).local_tunnel(handle, box_port)
    }

    fn batch(
        &self,
        handle: &BoxHandle,
        connect: &[String],
        place: Place,
        steps: &[BoxStep],
        deadline: Deadline,
    ) -> Vec<Result<StepDone, DriverError>> {
        self.of(handle)
            .batch(handle, connect, place, steps, deadline)
    }

    fn launch_run(
        &self,
        handle: &BoxHandle,
        run: i64,
        spec: &ExecSpec,
    ) -> Result<Launched, DriverError> {
        self.of(handle).launch_run(handle, run, spec)
    }

    fn follow_run(
        &self,
        handle: &BoxHandle,
        run: i64,
        from: u64,
        out: &mut dyn std::io::Write,
        deadline: Deadline,
    ) -> Result<Followed, DriverError> {
        self.of(handle).follow_run(handle, run, from, out, deadline)
    }

    fn end_run(
        &self,
        handle: &BoxHandle,
        run: i64,
        workdir: Option<&BoxPath>,
    ) -> Result<RunEnd, DriverError> {
        self.of(handle).end_run(handle, run, workdir)
    }

    fn cancel_run(
        &self,
        handle: &BoxHandle,
        run: i64,
        grace_secs: u32,
    ) -> Result<Cancelled, DriverError> {
        self.of(handle).cancel_run(handle, run, grace_secs)
    }

    fn sleep(&self, handle: &BoxHandle) -> Result<SleepOutcome, DriverError> {
        self.of(handle).sleep(handle)
    }

    fn wake(&self, handle: &BoxHandle) -> Result<(), DriverError> {
        self.of(handle).wake(handle)
    }

    fn touch(&self, handle: &BoxHandle) -> Result<(), DriverError> {
        self.of(handle).touch(handle)
    }
}

#[cfg(test)]
mod tests;
