//! The Box user, read from the environment of a process on the Box.

use moor_core::name::BoxPath;
use moor_core::seam::driver::StepDone;

use super::install::StepResult;
use super::{fail, Failure};

/// The Box user's home, and whether that user is root, read from the environment of a process on
/// the Box.
pub(super) fn box_user(peeked: Option<StepResult>) -> Result<(BoxPath, bool), Failure> {
    let text = match peeked {
        Some(Ok(StepDone::Peeked(Some(text)))) => text,
        Some(Err(e)) => return Err(fail("home", &e)),
        _ => return Err(fail("home", &"the Box has no process environment")),
    };
    Ok((home_in(&text)?, environ_var(&text, "USER") == Some("root")))
}

/// The value of `name` in NUL-separated process environment `text`.
fn environ_var<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!("{name}=");
    text.split('\0').find_map(|kv| kv.strip_prefix(&prefix))
}

/// The Box user's home from NUL-separated process environment `text`.
pub(super) fn home_in(text: &str) -> Result<BoxPath, Failure> {
    let home = environ_var(text, "HOME")
        .ok_or_else(|| fail("home", &"the Box user has no home directory"))?;
    BoxPath::restore(home).map_err(|e| fail("home", &e))
}
