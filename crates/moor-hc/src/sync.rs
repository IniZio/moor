//! One sync pass: plan hc's machines against the up Boxes and make the changes. A pass that
//! fails part way is finished by the next one.

use moor_client::Client;

use crate::boxes::up_boxes;
use crate::error::Error;
use crate::hc::Hc;
use crate::plan::plan;
use crate::via::Link;

/// Brings hc's machines in line with the up Boxes behind `client`, each reached by `exe` over
/// `link`. Returns one line per change made or failed, for the log.
pub fn once(
    client: &Client,
    hc: &mut dyn Hc,
    exe: &str,
    link: &Link,
) -> Result<Vec<String>, Error> {
    let boxes = up_boxes(client)?;
    let steps = plan(exe, link, &boxes, &hc.machines()?);
    Ok(steps
        .iter()
        .map(|step| match hc.apply(step) {
            Ok(()) => step.to_string(),
            Err(e) => format!("{step} failed: {e}"),
        })
        .collect())
}
