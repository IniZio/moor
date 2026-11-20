//! Waiting for a Box's places to answer, and the files written into the agent place.

use super::cache;
use super::cli::{Cli, State};
use super::names::Names;
use super::{Docker, AGENT_READY_TRIES, STOP_SECONDS, WORK_READY_TRIES};
use crate::boxes::provider::ProvideError;
use crate::boxes::Slug;

impl Docker {
    /// Counts the cache volumes the work place `work` mounts as used now, so a Box that ran long
    /// does not leave them looking old when it closes.
    async fn touch_mounted(&self, work: &str) {
        let format = "{{range .Mounts}}{{.Name}} {{end}}";
        let said = Cli::ok(["inspect", "-f", format, work])
            .await
            .unwrap_or_default();
        let names = said
            .split_whitespace()
            .filter(|n| n.starts_with("moor-cache-"));
        self.caches.uses.touch(names);
    }

    /// Waits for the agent place's init to make the key and the tools the work place starts from.
    pub(super) async fn await_agent(&self, names: &Names) -> Result<(), ProvideError> {
        for _ in 0..AGENT_READY_TRIES {
            let ready = ["exec", &names.agent, "test", "-e", "/run/moor/agent.ready"];
            if Cli::passes(ready).await {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        Err(ProvideError::Failed(
            "the agent place did not become ready".to_owned(),
        ))
    }

    /// Waits until a command in the work place runs, which is when its sshd answers.
    pub(super) async fn await_work(&self, names: &Names) -> Result<(), ProvideError> {
        for _ in 0..WORK_READY_TRIES {
            if Cli::passes(["exec", &names.agent, "box-shell", "-c", "true"]).await {
                return Ok(());
            }
            if Cli::state(&names.work).await? != Some(State::Running) {
                let logs = Cli::output(["logs", "--tail", "20", &names.work]).await?;
                let tail = String::from_utf8_lossy(&logs.stderr);
                return Err(ProvideError::Failed(format!(
                    "the work place stopped while starting: {}",
                    tail.trim()
                )));
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        Err(ProvideError::Failed(
            "the work place did not answer over ssh".to_owned(),
        ))
    }

    /// Stops the work place so nothing writes while its private cache volumes are read, then saves
    /// them to the canonical ones. A save that fails is said and never keeps a Box open.
    #[allow(clippy::print_stderr, reason = "the agent has no log but its stderr")]
    pub(super) async fn keep_caches(&self, slug: &Slug, names: &Names) -> Result<(), ProvideError> {
        self.touch_mounted(&names.work).await;
        if Cli::state(&names.work).await? == Some(State::Running) {
            Cli::ok(["stop", "-t", STOP_SECONDS, &names.work]).await?;
        }
        if let Err(e) = cache::save(&self.caches, &self.settings.agent_image, slug).await {
            eprintln!(
                "moor-machine-agent: cache of {} not saved: {e}",
                slug.as_str()
            );
        }
        Ok(())
    }

    /// Writes `content` to `path` in the agent place, through a rename so a reader never sees half.
    pub(super) async fn write_run_file(
        &self,
        names: &Names,
        path: &str,
        content: &str,
    ) -> Result<(), ProvideError> {
        let script = format!("cat > {path}.new && mv -f {path}.new {path}");
        Cli::feed(["exec", "-i", &names.agent, "sh", "-c", &script], content).await
    }

    /// The OOM kills the work place's memory limit has made, from the run volume.
    pub(super) async fn oom_kills(&self, names: &Names) -> u32 {
        Cli::ok(["exec", &names.agent, "cat", "/run/moor/oom"])
            .await
            .ok()
            .and_then(|text| text.parse().ok())
            .unwrap_or(0)
    }
}
