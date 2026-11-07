//! Whether a runner lets in an agent: one at or above the oldest release that speaks this link,
//! once the runner was built as a release.

use moor_client::link::{Frame, Offer};
use semver::Version;

/// The release this runner was built as; a build without one lets in any agent.
pub(super) const STAMPED: Option<&str> = option_env!("MOOR_VERSION");
/// The runner's release, which an agent told to update must become.
pub(super) const VERSION: &str = match STAMPED {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
/// The oldest agent release that speaks this link. Raise it by hand, to the first release that
/// speaks the new link, only when a change would break an older agent.
pub(super) const MIN_AGENT_VERSION: &str = "0.3.0-nightly.191";

/// The machine, release and offer `hello` names when a runner built as `stamped` lets that
/// release in past `floor`, or the `update-required` frame naming the runner's own release.
pub(super) fn judge(
    stamped: Option<&str>,
    floor: &str,
    hello: &str,
) -> Result<(i64, String, Offer), Option<Frame>> {
    let own = stamped.unwrap_or(env!("CARGO_PKG_VERSION"));
    match serde_json::from_str::<Frame>(hello) {
        Ok(Frame::Hello {
            machine_id,
            version,
            offer,
        }) if stamped.is_none_or(|s| admits(s, floor, &version)) => Ok((
            machine_id,
            version,
            offer.map_or_else(Offer::default, |o| *o),
        )),
        _ => Err(Some(Frame::UpdateRequired {
            version: own.to_owned(),
        })),
    }
}

/// Whether a runner of release `own` links an agent of release `agent`: at or above `floor` by
/// semver precedence, or at or above `own` when the runner itself is older than `floor`.
fn admits(own: &str, floor: &str, agent: &str) -> bool {
    match (
        Version::parse(own),
        Version::parse(floor),
        Version::parse(agent),
    ) {
        (Ok(own), Ok(floor), Ok(agent)) => agent >= floor.min(own),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
