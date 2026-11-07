//! Opening the state store and checking the user config before the controller listens.

use super::{Stop, REFUSED, USAGE};

use crate::env::Env;
use crate::error::CommandError;
use crate::Moor;
use moor_core::ErrorKind;

/// The stop for a store that could not be opened or claimed.
pub(super) fn store_refused(e: &CommandError) -> Stop {
    Stop {
        code: match e.kind {
            ErrorKind::Refused => REFUSED,
            ErrorKind::Usage => USAGE,
            _ => 1,
        },
        message: e.error.to_string(),
    }
}

/// Opens the state store, then loads the user config once.
///
/// An invalid config fails the start, so a bad deploy crash-loops where it is seen instead of
/// listening and failing every request.
pub(super) fn open(env: Env) -> Result<Env, Stop> {
    let env = Moor::open_store(env).map_err(|e| store_refused(&e))?;
    Moor::from_env(&env).map_err(|e| Stop {
        code: 1,
        message: e.error.to_string(),
    })?;
    Ok(env)
}
