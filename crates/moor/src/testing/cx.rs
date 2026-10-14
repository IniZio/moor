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
use crate::testing::forge::FakeForge;
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
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
        Self::fake_with_clock(state_dir, cwd, driver, surface, proc, config, clock)
    }

    /// Test context like [`Cx::fake_with`] on a clock the caller also holds.
    #[allow(clippy::too_many_arguments, reason = "fake_with plus the clock")]
    pub(crate) fn fake_with_clock(
        state_dir: &Path,
        cwd: &Path,
        driver: Arc<dyn Driver>,
        surface: Arc<dyn Surface>,
        proc: Arc<dyn Proc>,
        config: UserConfig,
        clock: Arc<FakeClock>,
    ) -> Self {
        Self::new(CxParts {
            state_dir: Some(state_dir.to_path_buf()),
            clock,
            rng: Arc::new(FakeRng::default()),
            driver,
            surface,
            engine: Arc::new(FakeEngine::new()),
            forge: Arc::new(FakeForge::new()),
            git: Git::new(Arc::clone(&proc)),
            proc: Arc::clone(&proc),
            config,
            creds: Creds::new(Env::default(), proc),
            cwd: cwd.to_path_buf(),
            prompt_window: Duration::from_secs(8),
        })
    }
}
