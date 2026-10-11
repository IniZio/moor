use std::io::Write;
use std::path::Path;
use std::process::Stdio;

use super::github_ops::{GithubSetupOpts, GithubStore};
use super::old::{seam_env, OldCli};
use super::{exit_class, Outcome};
use crate::world::World;

fn setup_args(opts: &GithubSetupOpts) -> Vec<String> {
    let mut args: Vec<String> = ["github", "setup"].map(String::from).to_vec();
    let flags = [
        ("--app-id", opts.app_id.clone()),
        ("--client-id", opts.client_id.clone()),
        ("--key", opts.key.as_ref().map(|k| k.display().to_string())),
        ("--repo", opts.repo.clone()),
    ];
    for (flag, value) in flags {
        if let Some(v) = value {
            args.extend([flag.to_string(), v]);
        }
    }
    if opts.json {
        args.push("--json".into());
    }
    args.extend(opts.extra_args.iter().cloned());
    args
}

impl OldCli {
    fn github_call(
        &self,
        w: &World,
        cwd: &Path,
        args: &[String],
        env: &[(&str, &str)],
        stdin: Option<&str>,
    ) -> Outcome<GithubStore> {
        let mut cmd = w.command(&self.bin);
        cmd.envs(seam_env(w)).envs(env.iter().copied());
        cmd.current_dir(cwd)
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn moor");
        w.track_pid(child.id());
        if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
            let _ = pipe.write_all(text.as_bytes());
        }
        let out = child.wait_with_output().expect("wait for moor");
        let exit = out.status.code().unwrap_or(1);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        w.log_transcript(format!("moor {}\n{stdout}\n{stderr}", args.join(" ")));
        Outcome {
            exit,
            class: exit_class(exit),
            stdout,
            stderr,
            warnings: Vec::new(),
            value: Some(GithubStore::read(w)),
        }
    }

    pub(super) fn github_setup_impl(
        &self,
        w: &World,
        cwd: &Path,
        opts: &GithubSetupOpts,
    ) -> Outcome<GithubStore> {
        let env: Vec<(&str, &str)> = opts
            .secret_env
            .iter()
            .map(|s| ("MOOR_GITHUB_CLIENT_SECRET", s.as_str()))
            .collect();
        let stdin = opts.secret_stdin.as_deref();
        self.github_call(w, cwd, &setup_args(opts), &env, stdin)
    }

    pub(super) fn github_login_impl(&self, w: &World, cwd: &Path) -> Outcome<GithubStore> {
        let args = ["github", "login"].map(String::from);
        self.github_call(w, cwd, &args, &[], None)
    }
}

impl OldCli {
    pub(super) fn github_raw_impl(
        &self,
        w: &World,
        cwd: &Path,
        args: &[&str],
    ) -> Outcome<GithubStore> {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        self.github_call(w, cwd, &args, &[], None)
    }
}
