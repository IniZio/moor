//! Delivers the user's dotfiles into a freshly acquired Box.

use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::Duration;

use moor_core::boxscript::quote;
use moor_core::config::BoxSettings;
use moor_core::domain::run::SyncMode;
use moor_core::exec_env::Purpose;
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Output, ProcSpec, Tool};
use moor_core::view::DotfilesView;
use moor_core::{BoxHandle, BoxId, BoxPath};

use crate::cx::Cx;
use crate::ops::common::box_exec::{self, BoxProxy, Site};
use crate::ops::show::redact_nonce;

/// Wall-clock allowance for the install step inside the Box.
pub(crate) const DOTFILES_DEADLINE: Duration = Duration::from_mins(15);

/// Wall-clock allowance for packing the dotfiles directory on the host.
pub(crate) const TAR_DEADLINE: Duration = Duration::from_mins(2);

/// How many trailing log lines a failure reports.
pub(crate) const TAIL_LINES: usize = 20;

/// How the dotfiles step ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DotfilesRun {
    /// No dotfiles are configured.
    Skipped,
    /// The dotfiles were delivered and installed.
    Ok {
        /// Time the step took, in milliseconds.
        ms: u64,
    },
    /// The install failed; the Box carries on without the dotfiles.
    Failed {
        /// The last output lines, or the reason when there was no output.
        tail: String,
    },
}

impl DotfilesRun {
    /// The outcome shown to the user.
    pub(crate) fn view(&self) -> DotfilesView {
        match self {
            Self::Skipped => DotfilesView::default(),
            Self::Ok { ms } => DotfilesView {
                ran: true,
                ok: true,
                ms: Some(*ms),
            },
            Self::Failed { .. } => DotfilesView {
                ran: true,
                ok: false,
                ms: None,
            },
        }
    }
}

/// A failed dotfiles step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DotfilesFailed {
    /// The last output lines, or the reason when there was no output.
    pub(crate) tail: String,
}

/// The Box script that unpacks the archive at `archive` into `~/.moor-dotfiles` and runs the
/// install script: `install` relative to the dotfiles root, else `install.sh` when present.
/// The upload directory `stage` is removed on exit, whether or not the unpack succeeds.
pub(crate) fn box_script(stage: &str, archive: &str, install: Option<&str>) -> String {
    let qs = quote(stage);
    let qa = quote(archive);
    let tmp = "\"$HOME/.moor-dotfiles.moor-tmp\"";
    let dir = "\"$HOME/.moor-dotfiles\"";
    let install = install.map_or_else(
        || "if [ -f install.sh ]; then MOOR_DOTFILES=1 bash ./install.sh; fi".to_owned(),
        |x| {
            let qp = quote(&format!("./{x}"));
            let qm = quote(&format!("dotfiles install script {x} not found"));
            format!("[ -f {qp} ] || {{ echo {qm} >&2; exit 1; }}; MOOR_DOTFILES=1 bash {qp}")
        },
    );
    format!(
        "set -e; trap \"rm -rf {qs}\" EXIT; rm -rf {tmp}; mkdir -p {tmp}; tar -xzf {qa} -C {tmp}; \
         rm -rf {dir}; mv {tmp} {dir}; cd {dir}; {install}"
    )
}

/// The last [`TAIL_LINES`] lines of `log`, ignoring trailing blank lines.
fn tail(log: &str) -> String {
    let lines: Vec<&str> = log.trim_end().lines().collect();
    let skip = lines.len().saturating_sub(TAIL_LINES);
    lines.into_iter().skip(skip).collect::<Vec<_>>().join("\n")
}

/// Creates the host log owner-only, in an owner-only directory: install output may hold secrets.
fn private_log(log: &Path) -> std::io::Result<()> {
    if let Some(dir) = log.parent() {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        fs::set_permissions(dir, Permissions::from_mode(0o700))?;
    }
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(log)?;
    fs::set_permissions(log, Permissions::from_mode(0o600))
}

/// Runs the install `script` in the Box with the env and Proxy credentials setup gets, logging to
/// `log`. The error is why it could not run to an exit.
fn install(
    cx: &Cx,
    site: &Site<'_>,
    cfg: &BoxSettings,
    proxy: Option<&BoxProxy>,
    script: String,
    log: &Path,
) -> Result<ExecOutcome, String> {
    let env = box_exec::prepare(cx, site, proxy, cfg, Purpose::Run)
        .map_err(|e| format!("could not prepare the dotfiles install: {e}"))?;
    let exec = ExecSpec {
        argv: env.argv(vec!["sh".to_owned(), "-c".to_owned(), script]),
        env: env.vars(),
        cwd: None,
        sync: SyncMode::None,
        output: Some(Output::Log(log.to_path_buf())),
        host_cwd: Some(site.worktree.to_path_buf()),
        class: Some(cfg.size.value.as_str().to_owned()),
    };
    let out = cx
        .driver()
        .exec(site.handle, &exec, Deadline(DOTFILES_DEADLINE));
    if matches!(out, Err(_) | Ok(ExecOutcome::Interrupted)) {
        env.cleanup(cx, site);
    }
    out.map_err(|e| e.to_string())
}

/// Packs the configured dotfiles on the host, uploads them, and runs the install script in
/// the Box. Does nothing when no dotfiles are configured.
///
/// Symlinks inside the dotfiles directory are followed (`tar -h`), so the files they point to
/// are copied into the Box, even when they live outside that directory.
///
/// The install script runs with the env and Proxy credentials setup gets. `proxy` is how starting
/// the Proxy went; an `Err` fails the step before anything is packed.
pub(crate) fn apply(
    cx: &Cx,
    handle: &BoxHandle,
    id: BoxId,
    host_cwd: &Path,
    cfg: &BoxSettings,
    proxy: &Result<Option<BoxProxy>, String>,
) -> Result<DotfilesRun, DotfilesFailed> {
    let Some(d) = &cfg.dotfiles else {
        return Ok(DotfilesRun::Skipped);
    };
    let fail = |tail: String| DotfilesFailed { tail };
    let proxy = proxy
        .as_ref()
        .map_err(|e| fail(format!("the Proxy could not start: {e}")))?;
    let started = cx.clock().instant();
    let src = cx.creds().env().expand_home(&d.path);
    let spec = ProcSpec::new(Tool::Sh, Deadline(TAR_DEADLINE))
        .args(["-c", "exec tar -C \"$1\" -czhf - .", "sh"])
        .arg(src);
    let bytes = cx
        .proc()
        .run_secret(&spec)
        .map_err(|e| fail(format!("could not pack dotfiles: {e}")))?;
    let n = id.get();
    let (hi, lo) = (cx.rng().u32(), cx.rng().u32());
    let stage = format!("/tmp/moor-dotfiles-{n}-{hi:08x}{lo:08x}");
    let archive = BoxPath::under(&stage, "dotfiles.tgz")
        .map_err(|e| fail(format!("could not upload dotfiles: {e}")))?;
    cx.driver()
        .put_file(handle, &archive, &bytes, 0o600, host_cwd)
        .map_err(|e| fail(format!("could not upload dotfiles: {e}")))?;
    let log = cx
        .state_dir()
        .map_err(|e| fail(e.error.to_string()))?
        .join("dotfiles")
        .join(format!("{n}.log"));
    private_log(&log).map_err(|e| fail(format!("could not create dotfiles log: {e}")))?;
    let script = box_script(&stage, archive.as_str(), d.install.as_deref());
    let site = Site {
        handle,
        provider: &cfg.provider.value,
        workdir: None,
        worktree: host_cwd,
    };
    let reason = match install(cx, &site, cfg, proxy.as_ref(), script, &log) {
        Ok(ExecOutcome::Exited { code: 0 }) => {
            let elapsed = cx.clock().instant().saturating_duration_since(started);
            let ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
            return Ok(DotfilesRun::Ok { ms });
        }
        Ok(ExecOutcome::Exited { code }) => format!("exit {code}"),
        Ok(ExecOutcome::Interrupted) => "interrupted".to_owned(),
        Err(e) => e,
    };
    let logged = std::fs::read_to_string(&log)
        .map(|s| tail(&redact_nonce(&s, None)))
        .unwrap_or_default();
    Err(fail(if logged.is_empty() { reason } else { logged }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAGE: &str = "/tmp/moor-dotfiles-1-0123456789abcdef";
    const ARCHIVE: &str = "/tmp/moor-dotfiles-1-0123456789abcdef/dotfiles.tgz";
    const HEAD: &str = "set -e; trap \"rm -rf '/tmp/moor-dotfiles-1-0123456789abcdef'\" EXIT; rm -rf \"$HOME/.moor-dotfiles.moor-tmp\"; mkdir -p \"$HOME/.moor-dotfiles.moor-tmp\"; tar -xzf '/tmp/moor-dotfiles-1-0123456789abcdef/dotfiles.tgz' -C \"$HOME/.moor-dotfiles.moor-tmp\"; rm -rf \"$HOME/.moor-dotfiles\"; mv \"$HOME/.moor-dotfiles.moor-tmp\" \"$HOME/.moor-dotfiles\"; cd \"$HOME/.moor-dotfiles\"; ";

    #[test]
    fn script_defaults_to_install_sh() {
        let want =
            format!("{HEAD}if [ -f install.sh ]; then MOOR_DOTFILES=1 bash ./install.sh; fi");
        assert_eq!(box_script(STAGE, ARCHIVE, None), want);
    }

    #[test]
    fn script_runs_the_named_install_script() {
        let want = format!(
            "{HEAD}[ -f './my setup.sh' ] || {{ echo 'dotfiles install script my setup.sh not found' >&2; exit 1; }}; MOOR_DOTFILES=1 bash './my setup.sh'"
        );
        assert_eq!(box_script(STAGE, ARCHIVE, Some("my setup.sh")), want);
    }

    #[test]
    fn view_reports_each_outcome() {
        assert_eq!(DotfilesRun::Skipped.view(), DotfilesView::default());
        assert_eq!(
            DotfilesRun::Ok { ms: 42 }.view(),
            DotfilesView {
                ran: true,
                ok: true,
                ms: Some(42)
            }
        );
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        let log = (1..=30).fold(String::new(), |acc, i| acc + &format!("l{i}\n"));
        let got = tail(&format!("{log}\n\n"));
        assert_eq!(got.lines().count(), TAIL_LINES);
        assert!(got.starts_with("l11\n"));
        assert!(got.ends_with("l30"));
        assert_eq!(tail(""), "");
    }
}
