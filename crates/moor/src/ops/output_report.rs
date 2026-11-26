//! `output-report`: something the agent in a Box made outside moor, kept for its run and task pages.

use moor_core::output::{Output, PER_BOX_MAX};
use moor_core::request::OutputReportReq;
use moor_core::view::output::OutputReportValue;
use moor_core::ErrorKind;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::outputs::Kept;

/// Keeps the reported output of `req.box_id`, or says why not.
pub(crate) fn run(cx: &Cx, req: &OutputReportReq) -> Result<OutputReportValue, CommandError> {
    let output = Output::parse(&req.kind, &req.url, &req.title)
        .map_err(|why| CommandError::new(ErrorKind::Usage, why))?;
    let store = cx.store()?;
    if store.box_row(req.box_id)?.is_none() {
        return Err(CommandError::new(
            ErrorKind::Refused,
            format!("no Box {}", req.box_id.get()),
        ));
    }
    let reply = match store.put_output(req.box_id, &output, cx.clock().now())? {
        Kept::Added => format!("kept {} {}", output.kind().as_str(), output.url()),
        Kept::Updated => format!("updated {} {}", output.kind().as_str(), output.url()),
        Kept::Full => {
            return Err(CommandError::new(
                ErrorKind::Refused,
                format!("this Box already has {PER_BOX_MAX} outputs"),
            ))
        }
    };
    Ok(OutputReportValue { reply })
}
