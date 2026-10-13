//! Test context builder.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::config::UserConfig;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::Driver;
use moor_core::seam::proc::Proc;
use moor_core::seam::surface::Surface;

use crate::adapters::creds::Creds;
use crate::adapters::git::Git;
use crate::cx::{Cx, CxParts};
use crate::env::Env;
use crate::testing::clock::FakeClock;
use crate::testing::engine::FakeEngine;
use crate::testing::rng::FakeRng;

impl Cx {
    /// Test context: fake clock, a fake engine, default config, git over `proc`.
    pub(crate) fn fake(
        state_dir: &Path,
        cwd: &Path,
        driver: Arc<dyn Driver>,
        surface: Arc<dyn Surface>,
        proc: Arc<dyn Proc>,
    ) -> Self {
        Self::fake_with(state_dir, cwd, driver, surface, proc, UserConfig::default())
    }

    /// Test context like [`Cx::fake`] with user config `config`.
    pub(crate) fn fake_with(
        state_dir: &Path,
        cwd: &Path,
        driver: Arc<dyn Driver>,
        surface: Arc<dyn Surface>,
        proc: Arc<dyn Proc>,
        config: UserConfig,
    ) -> Self {
        Self::new(CxParts {
            state_dir: Some(state_dir.to_path_buf()),
            clock: Arc::new(FakeClock::new(Ts::from_millis(1000))),
            rng: Arc::new(FakeRng::default()),
            driver,
            surface,
            engine: Arc::new(FakeEngine::new()),
            git: Git::new(Arc::clone(&proc)),
            proc: Arc::clone(&proc),
            config,
            creds: Creds::new(Env::default(), proc),
            cwd: cwd.to_path_buf(),
            prompt_window: Duration::from_secs(8),
        })
    }
}
