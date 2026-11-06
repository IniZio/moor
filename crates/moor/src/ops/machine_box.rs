//! `machine-box`: the runner's record of each Box it set up on a machine.

use moor_core::request::MachineBoxOp;
use moor_core::view::user::MachineBoxValue;

use crate::cx::Cx;
use crate::error::CommandError;

/// Gets, puts, drops or lists records as `op` says; what it read or wrote.
pub(super) fn run(cx: &Cx, op: MachineBoxOp) -> Result<MachineBoxValue, CommandError> {
    let store = cx.store()?;
    let boxes = match op {
        MachineBoxOp::Get { box_id } => store.machine_box(box_id)?.into_iter().collect(),
        MachineBoxOp::Put(record) => {
            store.put_machine_box(&record)?;
            vec![record]
        }
        MachineBoxOp::Drop { box_id } => {
            store.drop_machine_box(box_id)?;
            Vec::new()
        }
        MachineBoxOp::List { machine_id } => store.machine_boxes(machine_id)?,
    };
    Ok(MachineBoxValue { boxes })
}
