//! Conversions from store and name errors to the command error.

use super::CommandError;
use crate::store::StoreError;
use moor_core::{ErrorKind, NameError};

impl From<StoreError> for CommandError {
    /// `Illegal` is a moor bug; a taken name, a state database from another schema version
    /// or a sleep blocked by busy Sessions is a refusal;
    /// every other store failure is a plain failure.
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Illegal(_) => Self::new(ErrorKind::Internal, e),
            StoreError::NameTaken(_)
            | StoreError::Schema { .. }
            | StoreError::SleepBlocked { .. } => Self::new(ErrorKind::Refused, e),
            other => Self::new(ErrorKind::Failed, format!("state store error: {other}")),
        }
    }
}

impl From<NameError> for CommandError {
    /// A rejected name is a usage error.
    fn from(e: NameError) -> Self {
        Self::new(ErrorKind::Usage, e)
    }
}
