//! Box teardown script.

use super::setup::steps_script;

#[cfg(test)]
mod tests;

/// POSIX sh text that runs `steps` like [`super::setup::setup_script`] does, logging to
/// `.moor/teardown.log` and `.moor/teardown.rc` so the setup log of the same Box survives.
#[must_use]
pub fn teardown_script(steps: &[String]) -> String {
    steps_script("teardown", steps)
}
