//! A user's own machines: `machine-add`, `machine-remove`, `machine-list`.

use std::future::Future;

use moor_client::link::broker;
use moor_core::config::{EnrolledMachine, UserMachine};
use moor_core::request::UserCmd;
use moor_core::user::{Email, Identity, UserId};
use moor_core::view::user::{
    MachineChangeValue, MachineEnrollValue, MachineListValue, MachineView,
};
use moor_core::ErrorKind;

use super::to_value;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::Store;

/// Run the machine request `cmd` for `user`.
pub(super) fn run(
    store: &Store,
    user: UserId,
    cmd: &UserCmd,
) -> Result<serde_json::Value, CommandError> {
    match cmd {
        UserCmd::MachineAdd {
            name,
            ssh,
            labels,
            capacity,
            pool,
        } => add(store, user, name, ssh, labels, (*capacity, pool.as_deref())),
        UserCmd::MachineRemove { name } => remove(store, user, name),
        UserCmd::MachineList => list(store, user),
        _ => Err(CommandError::new(ErrorKind::Usage, "not a machine request")),
    }
}

/// `machine-add`: check the machine, then store it over any of the same name.
pub(super) fn add(
    store: &Store,
    user: UserId,
    name: &str,
    ssh: &str,
    labels: &[String],
    (capacity, pool): (u16, Option<&str>),
) -> Result<serde_json::Value, CommandError> {
    let machine = UserMachine::parse(name, ssh, labels)
        .and_then(|m| m.with_capacity(capacity))
        .and_then(|m| match pool {
            Some(provider) => m.with_pool(provider),
            None => Ok(m),
        })
        .map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
    store.set_user_machine(user, &machine)?;
    to_value(&MachineChangeValue {
        name: machine.name,
        changed: true,
    })
}

/// `machine-enroll`: find or create `who`'s user by `email`, then store the machine over any
/// of the same name, replacing its key.
pub(super) fn enroll(
    store: &Store,
    who: &Identity,
    email: Option<&Email>,
    (name, pubkey, labels): (&str, &str, &[String]),
    (capacity, pool): (u16, Option<&str>),
) -> Result<serde_json::Value, CommandError> {
    let machine = EnrolledMachine::parse(name, pubkey, labels, capacity)
        .and_then(|m| match pool {
            Some(provider) => m.with_pool(provider),
            None => Ok(m),
        })
        .map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
    let (user, _) = store.enroll(who, email)?;
    let machine_id = store.enroll_machine(user, &machine)?;
    to_value(&MachineEnrollValue {
        machine_id,
        name: machine.name,
        user,
        owner: email.cloned(),
        capacity: machine.capacity,
    })
}

/// `machine-remove`: drop the machine; `changed` is false when there was none.
pub(super) fn remove(
    store: &Store,
    user: UserId,
    name: &str,
) -> Result<serde_json::Value, CommandError> {
    let changed = store.delete_user_machine(user, name)?;
    to_value(&MachineChangeValue {
        name: name.to_owned(),
        changed,
    })
}

/// `machine-list`: names, ssh targets, labels and capacities.
pub(super) fn list(store: &Store, user: UserId) -> Result<serde_json::Value, CommandError> {
    let mut links: std::collections::BTreeMap<_, _> =
        store.user_machine_links(user)?.into_iter().collect();
    let machines = store
        .user_machines(user)?
        .into_iter()
        .map(|m| MachineView {
            link: links.remove(&m.name),
            name: m.name,
            ssh: m.ssh,
            labels: m.labels,
            capacity: m.capacity,
            pool: m.pool,
        })
        .collect();
    to_value(&MachineListValue { machines })
}

/// `machine-status`: ask the agent of `user`'s machine `name` for its status through the
/// runner's links broker. Another user's machine is as unknown as one that does not exist.
pub(super) fn status(cx: &Cx, user: UserId, name: &str) -> Result<serde_json::Value, CommandError> {
    let store = cx.store()?;
    let refused = |reason: String| CommandError::new(ErrorKind::Refused, reason);
    if !store.user_machines(user)?.iter().any(|m| m.name == name) {
        return Err(refused(format!("you have no machine named {name}")));
    }
    let link = store
        .user_machine_links(user)?
        .into_iter()
        .find_map(|(n, link)| (n == name).then_some(link))
        .ok_or_else(|| refused(format!("machine {name} has no agent linked")))?;
    if !link.online {
        return Err(refused(format!(
            "machine {name} is offline since {}",
            crate::stamp::utc(link.since.as_millis() / 1000)
        )));
    }
    let socket = crate::runner_dir::of(cx.state_dir()?).join(broker::SOCKET);
    let status = blocking(broker::status(&socket, link.machine_id))
        .map_err(|e| CommandError::new(ErrorKind::Failed, format!("machine {name}: {e}")))?;
    to_value(&status)
}

/// Runs `call` to completion on a runtime of its own, off any runtime the caller is on.
fn blocking<T: Send>(call: impl Future<Output = Result<T, String>> + Send) -> Result<T, String> {
    std::thread::scope(|s| {
        s.spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?
                .block_on(call)
        })
        .join()
        .unwrap_or_else(|_| Err("the links broker call panicked".to_owned()))
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod status_tests;
