//! What `set_env` sends a machine: each Box's environment, once per change.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use moor_client::link::boxes::{BoxAnswer, BoxOp};
use moor_core::name::BoxHandle;
use moor_core::seam::driver::DriverError;

use super::{unexpected, Machines};

/// The environment of a Box as the machine holds it: variables and the CA.
type SentEnv = (Vec<(String, String)>, Option<String>);

/// The environments sent so far, by Box slug, kept as long as the controller runs.
#[derive(Default)]
pub struct SentEnvs(Mutex<HashMap<String, SentEnv>>);

/// The environments sent so far.
fn sent(m: &Machines) -> MutexGuard<'_, HashMap<String, SentEnv>> {
    m.env_sent.0.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Drops what was sent for `slug` when `closed` says its Box is gone.
pub(super) fn forget_closed(m: &Machines, slug: &str, closed: &Result<BoxAnswer, DriverError>) {
    if matches!(closed, Ok(BoxAnswer::Done | BoxAnswer::Gone)) {
        sent(m).remove(slug);
    }
}

/// Gives the Box `handle` names its environment, unless the machine already holds it.
pub(super) fn send(
    m: &Machines,
    handle: &BoxHandle,
    vars: &[(String, String)],
    ca_pem: Option<&str>,
) -> Result<(), DriverError> {
    let (machine, slug) = Machines::seat(handle)?;
    let want: SentEnv = (vars.to_vec(), ca_pem.map(str::to_owned));
    if sent(m).get(slug) == Some(&want) {
        return Ok(());
    }
    let op = BoxOp::Env {
        slug: slug.to_owned(),
        vars: want.0.clone(),
        ca_pem: want.1.clone(),
    };
    match m.call_again(machine, &op, &mut |_| {})? {
        BoxAnswer::Done => {
            sent(m).insert(slug.to_owned(), want);
            Ok(())
        }
        BoxAnswer::Gone => Err(DriverError::NotFound),
        other => Err(unexpected(&other)),
    }
}
