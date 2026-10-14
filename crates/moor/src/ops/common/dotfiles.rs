//! Delivers the user's dotfiles into a freshly acquired Box.

use std::path::Path;
use std::time::Duration;

use moor_core::boxscript::quote;
use moor_core::config::TaskSettings;
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Output, ProcSpec, Tool};
use moor_core::view::DotfilesView;
use moor_core::{BoxHandle, BoxPath, TaskId};

use crate::cx::Cx;

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
pub(crate) fn box_script(archive: &str, install: Option<&str>) -> String {
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
        "set -e; rm -rf {tmp}; mkdir -p {tmp}; tar -xzf {qa} -C {tmp}; rm -f {qa}; \
         rm -rf {dir}; mv {tmp} {dir}; cd {dir}; {install}"
    )
}

/// The last [`TAIL_LINES`] lines of `log`, ignoring trailing blank lines.
fn tail(log: &str) -> String {
    let lines: Vec<&str> = log.trim_end().lines().collect();
    let skip = lines.len().saturating_sub(TAIL_LINES);
    lines.into_iter().skip(skip).collect::<Vec<_>>().join("\n")
}

/// Packs the configured dotfiles on the host, uploads them, and runs the install script in
/// the Box. Does nothing when no dotfiles are configured.
pub(crate) fn apply(
    cx: &Cx,
    handle: &BoxHandle,
    id: TaskId,
    host_cwd: &Path,
    cfg: &TaskSettings,
) -> Result<DotfilesRun, DotfilesFailed> {
    let Some(d) = &cfg.dotfiles else {
        return Ok(DotfilesRun::Skipped);
    };
    let fail = |tail: String| DotfilesFailed { tail };
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
    let archive = BoxPath::under("/tmp", &format!("moor-dotfiles-{n}.tgz"))
        .map_err(|e| fail(format!("could not upload dotfiles: {e}")))?;
    cx.driver()
        .put_file(handle, &archive, &bytes, 0o600, host_cwd)
        .map_err(|e| fail(format!("could not upload dotfiles: {e}")))?;
    let log = cx
        .state_dir()
        .map_err(|e| fail(e.error.to_string()))?
        .join("dotfiles")
        .join(format!("{n}.log"));
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| fail(format!("could not create dotfiles log directory: {e}")))?;
    }
    let exec = ExecSpec {
        argv: vec![
            "sh".to_owned(),
            "-c".to_owned(),
            box_script(archive.as_str(), d.install.as_deref()),
        ],
        env: vec![],
        cwd: None,
        sync: SyncMode::None,
        output: Some(Output::Log(log.clone())),
        host_cwd: Some(host_cwd.to_path_buf()),
        class: Some(cfg.size.value.as_str().to_owned()),
    };
    let reason = match cx.driver().exec(handle, &exec, Deadline(DOTFILES_DEADLINE)) {
        Ok(ExecOutcome::Exited { code: 0 }) => {
            let elapsed = cx.clock().instant().saturating_duration_since(started);
            let ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
            return Ok(DotfilesRun::Ok { ms });
        }
        Ok(ExecOutcome::Exited { code }) => format!("exit {code}"),
        Ok(ExecOutcome::Interrupted) => "interrupted".to_owned(),
        Err(e) => e.to_string(),
    };
    let logged = std::fs::read_to_string(&log)
        .map(|s| tail(&s))
        .unwrap_or_default();
    Err(fail(if logged.is_empty() { reason } else { logged }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARCHIVE: &str = "/tmp/moor-dotfiles-1.tgz";
    const HEAD: &str = "set -e; rm -rf \"$HOME/.moor-dotfiles.moor-tmp\"; mkdir -p \"$HOME/.moor-dotfiles.moor-tmp\"; tar -xzf '/tmp/moor-dotfiles-1.tgz' -C \"$HOME/.moor-dotfiles.moor-tmp\"; rm -f '/tmp/moor-dotfiles-1.tgz'; rm -rf \"$HOME/.moor-dotfiles\"; mv \"$HOME/.moor-dotfiles.moor-tmp\" \"$HOME/.moor-dotfiles\"; cd \"$HOME/.moor-dotfiles\"; ";

    #[test]
    fn script_defaults_to_install_sh() {
        let want =
            format!("{HEAD}if [ -f install.sh ]; then MOOR_DOTFILES=1 bash ./install.sh; fi");
        assert_eq!(box_script(ARCHIVE, None), want);
    }

    #[test]
    fn script_runs_the_named_install_script() {
        let want = format!(
            "{HEAD}[ -f './my setup.sh' ] || {{ echo 'dotfiles install script my setup.sh not found' >&2; exit 1; }}; MOOR_DOTFILES=1 bash './my setup.sh'"
        );
        assert_eq!(box_script(ARCHIVE, Some("my setup.sh")), want);
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
