//! The record of a report moor refused: the timeline reads a refusal from it, never from the
//! name of the tool the agent called.

use moor_core::request::StepReportReq;
use moor_core::view::workflow::StepReportValue;
use moor_core::Warning;

use crate::cx::Cx;
use crate::error::CommandError;

/// `step-report`: records what the agent in `req.box_id` reported and tells it what to do next.
/// A refusal is recorded on the Box with its reply, so the timeline shows it as moor's.
///
/// # Errors
/// A store failure.
pub(crate) fn run(cx: &Cx, req: &StepReportReq) -> Result<StepReportValue, CommandError> {
    let reply = super::answer(cx, req)?;
    if reply.error {
        note(cx, req, &reply.reply);
    }
    Ok(reply)
}

/// Writes the `report_refused` event of the Box of `req` with `reply`. A failure is a warning:
/// the agent has its answer, and its timeline then shows the call as failed.
fn note(cx: &Cx, req: &StepReportReq, reply: &str) {
    let step = req.report.step.as_str();
    let noted = cx
        .store()
        .map_err(|e| e.error.to_string())
        .and_then(|store| {
            store
                .note_report_refused(req.box_id, cx.clock().now(), (step, reply))
                .map_err(|e| e.to_string())
        });
    if let Err(why) = noted {
        cx.warn(Warning {
            code: "report_refusal_unrecorded".to_owned(),
            message: format!("the refusal of the report of {step} is not recorded: {why}"),
        });
    }
}
