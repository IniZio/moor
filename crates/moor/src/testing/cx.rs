//! Test context builder.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::config::{parse_user, UserConfig};
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::Driver;
use moor_core::seam::proc::Proc;
use moor_core::seam::surface::Surface;
use moor_core::user::{Identity, UserId};
use moor_core::BoxName;

use crate::adapters::creds::Creds;
use crate::adapters::git::Git;
use crate::cx::{Cx, CxParts};
use crate::env::Env;
use crate::testing::clock::FakeClock;
use crate::testing::engine::FakeEngine;
use crate::testing::forge::FakeForge;
use crate::testing::proxy_host::FakeProxyHost;
use crate::testing::rng::FakeRng;

/// User config of the test context: only the credential the default Claude rule names.
pub(crate) fn fake_config() -> UserConfig {
    parse_user(
        "test",
        "credentials:\n  CLAUDE_CODE_OAUTH_TOKEN: {cmd: [echo, token]}\n",
    )
    .unwrap()
}

impl Cx {
    /// Test context: fake clock, a fake engine, one default credential, git over `proc`.
    pub(crate) fn fake(
        state_dir: &Path,
        cwd: &Path,
        driver: Arc<dyn Driver>,
        surface: Arc<dyn Surface>,
        proc: Arc<dyn Proc>,
    ) -> Self {
        Self::fake_with(state_dir, cwd, driver, surface, proc, fake_config())
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
        let cx = Self::bare(state_dir, cwd, (driver, surface, proc), config, clock);
        with_requester(cx, state_dir)
    }

    /// Test context like [`Cx::fake`] that acts as no one and has not opened its store.
    pub(crate) fn fake_bare(
        state_dir: &Path,
        cwd: &Path,
        driver: Arc<dyn Driver>,
        surface: Arc<dyn Surface>,
        proc: Arc<dyn Proc>,
    ) -> Self {
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
        Self::bare(
            state_dir,
            cwd,
            (driver, surface, proc),
            fake_config(),
            clock,
        )
    }

    /// The context over fakes, before it acts as anyone.
    fn bare(
        state_dir: &Path,
        cwd: &Path,
        (driver, surface, proc): (Arc<dyn Driver>, Arc<dyn Surface>, Arc<dyn Proc>),
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
            proxy_host: Arc::new(FakeProxyHost::new()),
            git: Git::new(Arc::clone(&proc)),
            proc: Arc::clone(&proc),
            config,
            creds: Creds::new(Env::default(), proc),
            cwd: cwd.to_path_buf(),
            prompt_window: Duration::from_secs(8),
        })
    }
}

/// The user [`requester`] is enrolled as: the first in a fresh test store.
pub(crate) const REQUESTER_USER: UserId = UserId::new(1);

/// Inserts Box `name` of repo `r`, acting for [`REQUESTER_USER`].
pub(crate) fn new_box(name: BoxName) -> Decision {
    Decision::insert(NewBox {
        name,
        repo: "r".into(),
    })
    .requester(REQUESTER_USER)
}

/// The identity a test context's requests act as.
pub(crate) fn requester() -> Identity {
    Identity::local(1000)
}

/// `cx` acting as [`requester`], enrolled in its store, with a vault key path in the state
/// dir. Nothing reads the key, since the test Proxy host is a fake.
fn with_requester(cx: Cx, state_dir: &Path) -> Cx {
    if let Ok(store) = cx.store() {
        store
            .enroll(&requester())
            .expect("enroll the test requester");
    }
    cx.with_requester(requester())
        .with_vault_key(state_dir.join("vault.key"))
}
