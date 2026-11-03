//! Which Surface the controller wires, and the warning when the config overrides the environment.

use std::path::PathBuf;
use std::sync::Arc;
#[cfg(feature = "pty-surface")]
use std::sync::OnceLock;

use moor_core::config::{SurfaceKind, UserConfig};
use moor_core::seam::clock::Clock;
use moor_core::seam::proc::Proc;
use moor_core::seam::surface::Surface;
use moor_core::{ErrorKind, Warning};

use super::herdr;
#[cfg(feature = "pty-surface")]
use crate::adapters::pty::PtySurface;
use crate::env::Env;
use crate::error::CommandError;

/// The process's one pty surface: its panes must outlive each request's wiring.
#[cfg(feature = "pty-surface")]
fn pty_surface(state_dir: Option<PathBuf>) -> Arc<dyn Surface> {
    static PTY: OnceLock<Arc<PtySurface>> = OnceLock::new();
    let pty = PTY.get_or_init(|| Arc::new(PtySurface::default().with_cwd(state_dir)));
    Arc::clone(pty) as Arc<dyn Surface>
}

/// The environment variable that picks the Surface when the config sets no `surface.kind`.
pub(super) const SURFACE_ENV: &str = "MOOR_SURFACE";

/// The Surface kind `config` sets, else the one [`SURFACE_ENV`] names, else herdr.
pub(crate) fn surface_kind(config: &UserConfig, env: &Env) -> Result<SurfaceKind, CommandError> {
    if let Some(kind) = config.surface.kind {
        return Ok(kind);
    }
    match env.get(SURFACE_ENV).map(str::trim) {
        None | Some("") => Ok(SurfaceKind::Herdr),
        Some(name) => SurfaceKind::parse(name).ok_or_else(|| {
            CommandError::new(
                ErrorKind::Usage,
                format!("{SURFACE_ENV} is {name}, but it must be herdr or pty"),
            )
        }),
    }
}

/// The warning for a config `surface.kind` that overrides a different [`SURFACE_ENV`].
pub(super) fn surface_override_warning(config: &UserConfig, env: &Env) -> Option<Warning> {
    let kind = config.surface.kind?;
    let named = env.get(SURFACE_ENV).map(str::trim)?;
    let other = SurfaceKind::parse(named).filter(|k| *k != kind)?;
    Some(Warning {
        code: "surface_overridden".to_owned(),
        message: format!(
            "the config sets surface.kind {} and wins over {SURFACE_ENV}={}",
            kind.as_str(),
            other.as_str()
        ),
    })
}

/// The Surface of `kind`.
#[cfg_attr(
    feature = "pty-surface",
    allow(
        clippy::unnecessary_wraps,
        reason = "a build without the feature refuses `pty`"
    )
)]
pub(super) fn surface(
    kind: SurfaceKind,
    config: &UserConfig,
    proc: &Arc<dyn Proc>,
    extra_env: Vec<(String, String)>,
    clock: Arc<dyn Clock>,
    state_dir: Option<PathBuf>,
) -> Result<Arc<dyn Surface>, CommandError> {
    match kind {
        SurfaceKind::Herdr => Ok(Arc::new(herdr(
            proc,
            config.herdr_session(),
            extra_env,
            clock,
            state_dir,
        ))),
        #[cfg(feature = "pty-surface")]
        SurfaceKind::Pty => Ok(pty_surface(state_dir)),
        #[cfg(not(feature = "pty-surface"))]
        SurfaceKind::Pty => Err(CommandError::new(
            ErrorKind::Usage,
            "the surface is pty, but this moor was built without the pty-surface feature",
        )),
    }
}
