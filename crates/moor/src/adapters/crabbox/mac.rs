//! Acquire steps that differ on a macOS Box.

use std::path::Path;

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, Platform};
use moor_core::task_key::{refuse_inside_login_home, MacLayout, TaskKey};

use super::{parse, workdir, Crabbox};

impl Crabbox {
    /// The macOS layout for `req`, or `None` on a Linux Box.
    pub(super) fn acquire_layout(
        &self,
        req: &AcquireReq,
    ) -> Result<Option<MacLayout>, DriverError> {
        match self.platform() {
            Platform::Macos => self.mac_layout(req).map(Some),
            Platform::Linux => Ok(None),
        }
    }

    /// The workdir of a freshly warmed Box, checked against the login home on macOS.
    pub(super) fn acquired_workdir(
        &self,
        handle: &BoxHandle,
        layout: Option<&MacLayout>,
        warm: &parse::Warmup,
        req: &AcquireReq,
    ) -> Result<BoxPath, DriverError> {
        let warm_dir = warm
            .workdir
            .as_deref()
            .and_then(|d| BoxPath::restore(d).ok());
        match (layout, warm_dir) {
            (Some(l), dir) => self.checked_mac_workdir(handle, l, dir, req.worktree.as_deref()),
            (None, Some(dir)) => Ok(dir),
            (None, None) => workdir::probe(self, handle, req.worktree.as_deref()),
        }
    }

    /// Where a macOS Box's per-Task directories go.
    fn mac_layout(&self, req: &AcquireReq) -> Result<MacLayout, DriverError> {
        let root = self.work_root.as_deref().ok_or_else(|| DriverError::Failed {
            summary: "a macOS box needs CRABBOX_STATIC_WORK_ROOT set to a directory outside every login home".into(),
        })?;
        let root = BoxPath::restore(root).map_err(|_| DriverError::Failed {
            summary: "CRABBOX_STATIC_WORK_ROOT must be an absolute path".into(),
        })?;
        MacLayout::new(&root, &TaskKey::new(&req.name)).map_err(|e| DriverError::Failed {
            summary: format!("cannot lay out the macOS box directories: {e}"),
        })
    }

    /// Reports the workdir of a macOS Box, or releases it when it sits inside the login home.
    fn checked_mac_workdir(
        &self,
        handle: &BoxHandle,
        layout: &MacLayout,
        warm: Option<BoxPath>,
        host_cwd: Option<&Path>,
    ) -> Result<BoxPath, DriverError> {
        let outcome = workdir::probe_home(self, handle, host_cwd).and_then(|(probed, home)| {
            let dir = warm.unwrap_or(probed);
            refuse_inside_login_home(&layout.root, &home)
                .and_then(|()| refuse_inside_login_home(&dir, &home))
                .map(|()| dir)
                .map_err(|summary| DriverError::Failed { summary })
        });
        if outcome.is_err() {
            let _ = self.release(handle);
        }
        outcome
    }
}
