//! Conversion from the close gate's refusal to the command error.

use super::CommandError;
use moor_core::{decide::close::Refusal, CloseRefusal, ErrorDetail, ErrorKind};

impl From<Refusal> for CommandError {
    /// Every refusal is `Refused`; its text names the way out and its detail carries the kind.
    fn from(r: Refusal) -> Self {
        let detail = ErrorDetail::CloseRefused(CloseRefusal::from(&r));
        let mut e = Self::new(ErrorKind::Refused, r);
        e.detail = Some(Box::new(detail));
        e
    }
}
