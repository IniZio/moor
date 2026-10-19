//! Conversion of Driver failures into command errors.

use super::CommandError;
use moor_core::seam::driver::DriverError;
use moor_core::ErrorKind;

/// Hint shown when every Box slot is taken.
const CAPACITY_HINT: &str = "retry after another Box closes; `moor show` lists holders";

impl From<DriverError> for CommandError {
    fn from(err: DriverError) -> Self {
        match err {
            DriverError::Capacity { .. } => {
                Self::new(ErrorKind::Refused, err).with_hint(CAPACITY_HINT)
            }
            other => Self::new(ErrorKind::Failed, other),
        }
    }
}
