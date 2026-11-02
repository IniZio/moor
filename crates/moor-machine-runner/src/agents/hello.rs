//! Whether a runner lets in an agent: only one of its own release, once it was built as one.

use moor_client::link::{Frame, Offer};

/// The release this runner was built as; a build without one lets in any agent.
pub(super) const STAMPED: Option<&str> = option_env!("MOOR_VERSION");
/// The runner's release, which an agent told to update must become.
pub(super) const VERSION: &str = match STAMPED {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
/// The machine, release and offer `hello` names when a runner built as `stamped` lets that
/// release in, or the `update-required` frame naming the runner's own release.
pub(super) fn judge(
    stamped: Option<&str>,
    hello: &str,
) -> Result<(i64, String, Offer), Option<Frame>> {
    let own = stamped.unwrap_or(env!("CARGO_PKG_VERSION"));
    match serde_json::from_str::<Frame>(hello) {
        Ok(Frame::Hello {
            machine_id,
            version,
            offer,
        }) if stamped.is_none_or(|s| s == version) => {
            Ok((machine_id, version, offer.unwrap_or_default()))
        }
        _ => Err(Some(Frame::UpdateRequired {
            version: own.to_owned(),
        })),
    }
}

#[cfg(test)]
mod tests;
