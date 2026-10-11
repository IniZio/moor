use std::path::Path;
use std::process::Stdio;

use serde_json::Value;

use crate::adapter::old::seam_env;
use crate::adapter::{exit_class, ExitClass, Outcome};
use crate::world::World;

fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|s| 128 + s))
        .unwrap_or(1)
}

fn full_args(w: &World, args: &[String]) -> Vec<String> {
    let mut all = vec![
        "--json".to_string(),
        "--state-dir".to_string(),
        w.state_dir().to_string_lossy().into_owned(),
    ];
    all.extend(args.iter().cloned());
    all
}

fn class_of(envelope: Option<&Value>, exit: i32) -> ExitClass {
    match envelope
        .and_then(|j| j.get("class"))
        .and_then(Value::as_str)
    {
        Some("ok") => ExitClass::Ok,
        Some("failed") => ExitClass::Failed,
        Some("usage") => ExitClass::Usage,
        Some("refused") => ExitClass::Refused,
        _ => match exit {
            3 => ExitClass::Refused,
            other => exit_class(other),
        },
    }
}

fn warnings_of(envelope: Option<&Value>) -> Vec<String> {
    envelope
        .and_then(|j| j.get("warnings"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.get("message").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Run the new binary once and decode its v2 JSON envelope.
pub(crate) fn invoke(
    bin: &Path,
    w: &World,
    cwd: &Path,
    args: &[String],
    stdin: Option<&str>,
) -> Outcome<Value> {
    let all = full_args(w, args);
    let mut child = w
        .command(bin)
        .envs(seam_env(w))
        .current_dir(cwd)
        .args(&all)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn {}: {e}", bin.display()));
    w.track_pid(child.id());
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        use std::io::Write;
        let _ = pipe.write_all(text.as_bytes());
    }
    let out = child.wait_with_output().expect("wait for moor");
    let exit = exit_code(out.status);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    w.log_transcript(format!("moor {}\n{stdout}\n{stderr}", all.join(" ")));
    let envelope: Option<Value> = serde_json::from_str(&stdout).ok();
    let value = envelope
        .as_ref()
        .and_then(|j| j.get("value"))
        .filter(|v| !v.is_null())
        .cloned();
    Outcome {
        exit,
        class: class_of(envelope.as_ref(), exit),
        value,
        warnings: warnings_of(envelope.as_ref()),
        stdout,
        stderr,
    }
}

/// Spawn the new binary detached with null stdio; the pid is tracked.
pub(crate) fn spawn(bin: &Path, w: &World, cwd: &Path, args: &[String]) -> std::process::Child {
    let child = w
        .command(bin)
        .envs(seam_env(w))
        .current_dir(cwd)
        .args(full_args(w, args))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn {}: {e}", bin.display()));
    w.track_pid(child.id());
    child
}

/// Map an outcome's JSON value into a typed value, keeping everything else.
pub(crate) fn retype<T>(o: Outcome<Value>, f: impl FnOnce(Value) -> Option<T>) -> Outcome<T> {
    Outcome {
        exit: o.exit,
        class: o.class,
        stdout: o.stdout,
        stderr: o.stderr,
        warnings: o.warnings,
        value: o.value.and_then(f),
    }
}

/// Outcome for a seam the new binary does not implement yet; nothing is spawned.
pub(crate) fn unmapped<T>(what: &str) -> Outcome<T> {
    Outcome {
        exit: 1,
        class: ExitClass::Failed,
        stdout: String::new(),
        stderr: format!("NewCli: {what} not mapped yet"),
        warnings: Vec::new(),
        value: None,
    }
}
