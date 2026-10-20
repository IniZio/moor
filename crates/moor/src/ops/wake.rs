//! `wake`: resume an asleep Box.
//!
//! Holds the Box lock while the Box resumes.

use moor_core::view::sleep::SleepValue;
use moor_core::BoxReq;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::wake::wake_box;

/// Run `wake`.
pub(crate) fn run(cx: &Cx, req: &BoxReq) -> Result<SleepValue, CommandError> {
    wake_box(cx, req, "wake", |_| Ok(()))
}

#[cfg(test)]
mod tests;
