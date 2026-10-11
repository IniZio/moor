//! Conversion of Surface failures into command errors.

use super::CommandError;
use moor_core::seam::surface::SurfaceError;
use moor_core::ErrorKind;

impl From<SurfaceError> for CommandError {
    fn from(err: SurfaceError) -> Self {
        Self::new(ErrorKind::Failed, err)
    }
}
