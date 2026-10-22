//! Test context builder.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::config::UserConfig;
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::Driver;
use moor_core::seam::proc::Proc;
use moor_core::seam::surface::Surface;
use moor_core::user::{Identity, SecretName, UserId};
use moor_core::BoxName;
use moor_core::Secret;

use crate::adapters::git::Git;
use crate::cx::{Cx, CxParts};
use crate::env::Env;
use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::testing::engine::FakeEngine;
use crate::testing::forge::FakeForge;
use crate::testing::proxy_host::FakeProxyHost;
use crate::testing::rng::FakeRng;
use crate::vault::Vault;

impl Cx {
    /// Test context: fake clock, a fake engine, one default credential, git over `proc`.
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
            UserConfig::default(),
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
            env: Env::default(),
            prompt_window: Duration::from_secs(8),
        })
        .with_dir(Some(cwd.to_path_buf()))
    }
}

/// The user [`requester`] is enrolled as: the first in a fresh test store.
pub(crate) const REQUESTER_USER: UserId = UserId::new(1);

/// Inserts Box `name` of repo `r`, acting for [`REQUESTER_USER`].
pub(crate) fn new_box(name: BoxName) -> Decision {
    new_box_in(name, Path::new("r"))
}

/// Inserts Box `name` of the repo at `repo`, acting for [`REQUESTER_USER`].
pub(crate) fn new_box_in(name: BoxName, repo: &Path) -> Decision {
    Decision::insert(NewBox {
        name,
        repo: repo.display().to_string(),
    })
    .requester(REQUESTER_USER)
}

/// The identity a test context's requests act as.
pub(crate) fn requester() -> Identity {
    Identity::local(1000)
}

/// The secrets the test requester holds: every one the default Proxy rules inject.
pub(crate) const REQUESTER_SECRETS: [&str; 2] = ["claude", "github"];

/// `cx` acting as [`requester`], enrolled in its store with [`REQUESTER_SECRETS`] sealed by a
/// vault key in the state dir.
fn with_requester(cx: Cx, state_dir: &Path) -> Cx {
    let key = state_dir.join("vault.key");
    if let (Ok(store), Ok(vault)) = (cx.store(), Vault::load_or_create(&key)) {
        let (user, _) = store
            .enroll(&requester())
            .expect("enroll the test requester");
        for name in REQUESTER_SECRETS {
            put_secret(store, &vault, user, name, &format!("{name}-test-value"));
        }
    }
    cx.with_requester(requester()).with_vault_key(key)
}

/// Stores `value` as `user`'s secret `name`, sealed with `vault`.
pub(crate) fn put_secret(store: &Store, vault: &Vault, user: UserId, name: &str, value: &str) {
    let name = SecretName::parse(name).expect("secret name");
    let sealed = vault
        .seal(user, &name, &Secret::new(value.as_bytes().to_vec()))
        .expect("seal");
    store
        .set_user_secret(user, &name, &sealed)
        .expect("store the secret");
}

/// A fresh vault key at `dir/vault.key`, for a Proxy spec to name; returns its path.
pub(crate) fn test_vault_key(dir: &Path) -> PathBuf {
    let path = dir.join("vault.key");
    Vault::load_or_create(&path).expect("test vault key");
    path
}
