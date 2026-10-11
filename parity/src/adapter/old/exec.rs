use std::path::Path;
use std::process::Stdio;

use serde_json::Value;

use super::env::seam_env;
use super::OldCli;
use crate::adapter::{exit_class, EnvOpts, Event, ExitClass, Moor, Outcome};
use crate::world::World;

pub(super) fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|s| 128 + s))
        .unwrap_or(1)
}

impl OldCli {
    pub(super) fn spawn_bg(&self, w: &World, cwd: &Path, args: &[String]) -> std::process::Child {
        let child = w
            .command(&self.bin)
            .envs(seam_env(w))
            .current_dir(cwd)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn {}: {e}", self.bin.display()));
        w.track_pid(child.id());
        child
    }

    pub(super) fn run_unit(&self, w: &World, cwd: &Path, args: &[String]) -> Outcome<()> {
        let mut o = self.run(w, cwd, args, |_| ());
        if o.class == ExitClass::Ok {
            o.value = Some(());
        }
        o
    }

    pub(in crate::adapter) fn run<T>(
        &self,
        w: &World,
        cwd: &Path,
        args: &[String],
        parse: impl Fn(&Value) -> T,
    ) -> Outcome<T> {
        self.run_env(w, cwd, args, &EnvOpts::default(), parse)
    }

    pub(super) fn run_env<T>(
        &self,
        w: &World,
        cwd: &Path,
        args: &[String],
        env: &EnvOpts,
        parse: impl Fn(&Value) -> T,
    ) -> Outcome<T> {
        let mut cmd = w.command(&self.bin);
        cmd.envs(seam_env(w)).envs(env.set_env.iter().cloned());
        for k in &env.unset_env {
            cmd.env_remove(k);
        }
        let child = cmd
            .current_dir(cwd)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn {}: {e}", self.bin.display()));
        w.track_pid(child.id());
        let out = child.wait_with_output().expect("wait for moor");
        let exit = exit_code(out.status);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        w.log_transcript(format!("moor {}\n{stdout}\n{stderr}", args.join(" ")));
        let json: Option<Value> = serde_json::from_str(&stdout).ok();
        let warnings = json
            .as_ref()
            .and_then(|j| j.get("warnings"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        Outcome {
            exit,
            class: exit_class(exit),
            value: json.as_ref().map(parse),
            stdout,
            stderr,
            warnings,
        }
    }
}

pub(super) fn name_args(verb: &str, name: &str) -> Vec<String> {
    vec![verb.to_string(), name.to_string(), "--json".to_string()]
}

const RUN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

pub(super) fn crabbox_run_calls(w: &World) -> usize {
    w.calls("crabbox")
        .iter()
        .filter(|c| c.argv.first().map(String::as_str) == Some("run"))
        .count()
}

pub(super) fn read_all(
    mut r: impl std::io::Read + Send + 'static,
) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    })
}

impl OldCli {
    pub(super) fn spawn_wait(
        &self,
        w: &World,
        cwd: &Path,
        args: &[String],
        interrupt: bool,
    ) -> (i32, String, String) {
        let before = crabbox_run_calls(w);
        let mut child = w
            .command(&self.bin)
            .envs(seam_env(w))
            .current_dir(cwd)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn {}: {e}", self.bin.display()));
        let pid = child.id();
        w.track_pid(pid);
        let out = read_all(child.stdout.take().expect("stdout pipe"));
        let err = read_all(child.stderr.take().expect("stderr pipe"));
        let start = std::time::Instant::now();
        let mut signalled = !interrupt;
        let status = loop {
            if let Some(st) = child.try_wait().expect("poll moor") {
                break st;
            }
            if !signalled && crabbox_run_calls(w) > before {
                unsafe { libc::kill(pid as i32, libc::SIGINT) };
                signalled = true;
            }
            if start.elapsed() > RUN_DEADLINE {
                let _ = child.kill();
                let _ = child.wait();
                panic!("moor {args:?} did not finish within {RUN_DEADLINE:?}");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let (stdout, stderr) = (
            out.join().unwrap_or_default(),
            err.join().unwrap_or_default(),
        );
        w.log_transcript(format!("moor {}\n{stdout}\n{stderr}", args.join(" ")));
        (exit_code(status), stdout, stderr)
    }

    pub(super) fn task_events(&self, w: &World, name: &str) -> Vec<Event> {
        self.events(w, name)
    }
}

pub(super) fn plain<T>(exit: i32, stdout: String, stderr: String, value: Option<T>) -> Outcome<T> {
    Outcome {
        exit,
        class: exit_class(exit),
        stdout,
        stderr,
        warnings: Vec::new(),
        value,
    }
}
