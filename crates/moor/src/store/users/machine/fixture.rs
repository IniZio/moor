//! Test fixtures that enroll a machine without an agent.

use moor_core::config::{EnrolledMachine, UserMachine};
use moor_core::user::UserId;

use crate::store::{Store, StoreError};

impl Store {
    /// Enrolls `machine` for `user` under a placeholder key, replacing one of the same name.
    pub(crate) fn set_user_machine(
        &self,
        user: UserId,
        machine: &UserMachine,
    ) -> Result<i64, StoreError> {
        self.enroll_machine(
            user,
            &EnrolledMachine {
                name: machine.name.clone(),
                pubkey: "ssh-ed25519 fixture".to_owned(),
                labels: machine.labels.clone(),
                capacity: machine.capacity,
                pool: machine.pool.clone(),
            },
        )
    }
}
