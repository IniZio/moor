//! Conversion from the close gate's refusal to the command error.

use super::CommandError;
use moor_core::{decide::close::Refusal, ErrorKind};

impl From<Refusal> for CommandError {
    /// Every refusal is `Refused`; lost work also suggests the way out.
    fn from(r: Refusal) -> Self {
        let hint = matches!(r, Refusal::UnpushedWork { .. });
        let err = Self::new(ErrorKind::Refused, r);
        if hint {
            err.with_hint("commit and push, or pass --force")
        } else {
            err
        }
    }
}
