//! Running the host's crabbox.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use super::{box_dir, Pool, CACHE_CAP, POOL_DIR, TAIL};

/// crabbox on this host, for one lease or, with no slug, for the pool.
pub(super) struct Crabbox<'a> {
    /// The account's home.
    pub(super) home: &'a Path,
    /// The pool.
    pub(super) pool: &'a Pool,
    /// The lease's slug.
    pub(super) slug: Option<&'a str>,
}

impl Crabbox<'_> {
    /// Runs `crabbox VERB --provider P FLAG SLUG`, or `FLAG` alone with no slug, within
    /// `within`; its stdout when it exits with a code in `ok`.
    pub(super) async fn run(
        &self,
        verb: &str,
        flag: &str,
        within: Duration,
        ok: &[i32],
    ) -> Result<String, String> {
        self.run_with(verb, &[flag], within, ok, &[]).await
    }

    /// [`Self::run`] with `flags` in place of its one flag and `env` added to crabbox's.
    pub(super) async fn run_with(
        &self,
        verb: &str,
        flags: &[&str],
        within: Duration,
        ok: &[i32],
        env: &[(&str, String)],
    ) -> Result<String, String> {
        let base = self.home.join(POOL_DIR);
        let mine = self.slug.map(|slug| box_dir(self.home, slug));
        // Only a warmup makes a lease's dir, so a call on a lease that is gone leaves nothing
        // behind; crabbox itself says whether the lease exists.
        let (dir, ceiling) = match mine {
            Some(mine) if verb == "warmup" || mine.join("lease").is_dir() => {
                (mine.join("lease"), mine)
            }
            _ => (base.clone(), base),
        };
        let mut args = vec![verb, "--provider", &self.pool.provider];
        args.extend(flags);
        args.extend(self.slug);
        self.spawn(&args, (&dir, &ceiling), within, ok, env).await
    }

    /// Runs `crabbox ARGS` in `dir`, with git looking no further up than `ceiling`, within
    /// `within`; its stdout when it exits with a code in `ok`.
    pub(super) async fn spawn(
        &self,
        args: &[&str],
        place: (&Path, &Path),
        within: Duration,
        ok: &[i32],
        env: &[(&str, String)],
    ) -> Result<String, String> {
        let printed = self.spawn_both(args, place, within, ok, env).await?;
        Ok(printed.0)
    }

    /// Like [`Self::spawn`], with what it said on stderr as well.
    pub(super) async fn spawn_both(
        &self,
        args: &[&str],
        (dir, ceiling): (&Path, &Path),
        within: Duration,
        ok: &[i32],
        env: &[(&str, String)],
    ) -> Result<(String, String), String> {
        let verb = args.first().copied().unwrap_or_default();
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
        let mut cmd = Command::new(&self.pool.crabbox);
        cmd.args(args)
            .current_dir(dir)
            .env("HOME", self.home)
            .env("GIT_CEILING_DIRECTORIES", ceiling)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .envs(self.pool.cache_gb.map(|gb| (CACHE_CAP, gb.to_string())))
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let out = tokio::time::timeout(within, cmd.output())
            .await
            .map_err(|_| format!("crabbox {verb} ran past {}s", within.as_secs()))?
            .map_err(|e| format!("cannot run crabbox: {e}"))?;
        match out.status.code() {
            Some(code) if ok.contains(&code) => Ok((
                String::from_utf8_lossy(&out.stdout).into_owned(),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            )),
            code => {
                let err = String::from_utf8_lossy(&out.stderr);
                let cut = err.len().saturating_sub(TAIL);
                let tail = err.get(cut..).unwrap_or(&err).trim();
                Err(format!("crabbox {verb} failed ({code:?}): {tail}"))
            }
        }
    }
}
