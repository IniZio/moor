//! Conversion from the close gate's refusal to the command error.

use super::CommandError;
use moor_core::{decide::close::Refusal, ErrorKind};

impl From<Refusal> for CommandError {
    /// Every refusal is `Refused`; its text names the way out.
    fn from(r: Refusal) -> Self {
        Self::new(ErrorKind::Refused, r)
    }
}
