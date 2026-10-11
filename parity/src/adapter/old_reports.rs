use std::path::Path;
use std::process::Stdio;

use super::OldCli;
use crate::adapter::{exit_class, Outcome, ReportReq, ReportRow};
use crate::world::World;

impl OldCli {
    pub(super) fn report_impl(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        req: &ReportReq,
    ) -> Outcome<ReportRow> {
        let mut args: Vec<String> = ["report", name, "--kind", &req.kind, "--json"]
            .map(String::from)
            .to_vec();
        let mut flag = |f: &str, v: Option<String>| {
            if let Some(v) = v {
                args.extend([f.to_string(), v]);
            }
        };
        flag("--file", req.file.as_ref().map(|p| p.display().to_string()));
        flag("--link", req.link.clone());
        flag("--version", req.version.map(|n| n.to_string()));
        flag("--title", req.title.clone());
        self.run(w, cwd, &args, ReportRow::from_json)
    }

    pub(super) fn reports_impl(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
    ) -> Outcome<Vec<ReportRow>> {
        let args = ["reports", name, "--json"].map(String::from);
        self.run(w, cwd, &args, |j| {
            j.as_array()
                .map(|a| a.iter().map(ReportRow::from_json).collect())
                .unwrap_or_default()
        })
    }

    pub(super) fn version_impl(&self, w: &World) -> Outcome<String> {
        let out = w
            .command(&self.bin)
            .arg("--version")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run moor --version");
        let exit = out.status.code().unwrap_or(1);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let value = (exit == 0).then(|| stdout.trim().to_string());
        Outcome {
            exit,
            class: exit_class(exit),
            stdout,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            warnings: Vec::new(),
            value,
        }
    }
}
