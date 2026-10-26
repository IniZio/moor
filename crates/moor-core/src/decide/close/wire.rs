//! A close refusal as the typed detail of the error envelope.

use crate::response::CloseRefusal;

use super::Refusal;

impl From<&Refusal> for CloseRefusal {
    fn from(r: &Refusal) -> Self {
        match r {
            Refusal::AlreadyClosed => Self::AlreadyClosed,
            Refusal::UnsavedInBox {
                branch,
                ahead,
                dirty,
                ..
            } => Self::Unsaved {
                branch: branch.clone(),
                ahead: *ahead,
                dirty: *dirty,
            },
            Refusal::BoxUnchecked { reason } => Self::Unchecked {
                reason: reason.clone(),
            },
        }
    }
}
