//! Whether a Box's provisioning is still alive.

use moor_core::decide::intent::{age, provisioning_dead};
use moor_core::{ErrorDetail, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::BoxRow;

/// Refuses a Box whose provisioning owner died before recording a machine.
pub(crate) fn dead_provisioning(cx: &Cx, row: &BoxRow) -> Result<(), CommandError> {
    let (Some(intent), Some(at)) = (row.intent, row.intent_at) else {
        return Ok(());
    };
    let handle = row.boxed.as_ref().and_then(|b| b.handle.as_ref());
    let held = age(at, cx.clock().now());
    if !provisioning_dead(&intent, cx.proc().alive(intent.pid()), handle, held) {
        return Ok(());
    }
    let name = row.name.as_str();
    Err(CommandError::new(
        ErrorKind::Failed,
        format!("provisioning dead; run moor close {name}"),
    )
    .with_hint(format!("moor close {name}"))
    .with_detail(ErrorDetail::ProvisioningDead))
}
