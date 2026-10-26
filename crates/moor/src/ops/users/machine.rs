//! A user's own machines: `machine-add`, `machine-remove`, `machine-list`.

use moor_core::config::UserMachine;
use moor_core::request::UserCmd;
use moor_core::user::UserId;
use moor_core::view::user::{MachineChangeValue, MachineListValue, MachineView};
use moor_core::ErrorKind;

use super::to_value;
use crate::error::CommandError;
use crate::store::Store;

/// Run the machine request `cmd` for `user`.
pub(super) fn run(
    store: &Store,
    user: UserId,
    cmd: &UserCmd,
) -> Result<serde_json::Value, CommandError> {
    match cmd {
        UserCmd::MachineAdd { name, ssh, labels } => add(store, user, name, ssh, labels),
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
) -> Result<serde_json::Value, CommandError> {
    let machine = UserMachine::parse(name, ssh, labels)
        .map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
    store.set_user_machine(user, &machine)?;
    to_value(&MachineChangeValue {
        name: machine.name,
        changed: true,
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

/// `machine-list`: names, ssh targets and labels.
pub(super) fn list(store: &Store, user: UserId) -> Result<serde_json::Value, CommandError> {
    let machines = store
        .user_machines(user)?
        .into_iter()
        .map(|m| MachineView {
            name: m.name,
            ssh: m.ssh,
            labels: m.labels,
        })
        .collect();
    to_value(&MachineListValue { machines })
}

#[cfg(test)]
mod tests;
