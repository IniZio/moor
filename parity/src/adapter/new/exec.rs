use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::Value;

use crate::adapter::{ExitClass, Outcome};
use crate::world::World;

/// Seam environment for the new binary (design §12 names).
pub(crate) fn seam_env(w: &World) -> Vec<(String, String)> {
    let config = w.root().join("agent-config.json");
    if !config.exists() {
        let doc = serde_json::json!({
            "projects": { w.root().to_string_lossy(): { "hasTrustDialogAccepted": true } }
        });
        std::fs::write(&config, doc.to_string()).expect("write agent-config.json");
    }
    let path = |p: PathBuf| p.to_string_lossy().into_owned();
    let mut pairs = vec![
        ("MOOR_TEST", "1".to_string()),
        ("MOOR_AGENT_CONFIG", path(config)),
    ];
    for (var, tool) in [
        ("MOOR_CRABBOX_BIN", "crabbox"),
        ("MOOR_HERDR_BIN", "herdr"),
        ("MOOR_CLAUDE_BIN", "claude"),
        ("MOOR_GH_BIN", "gh"),
        ("MOOR_SSH_BIN", "ssh"),
    ] {
        pairs.push((var, path(w.tool_path(tool))));
    }
    let preset = w.base_env();
    for var in ["MOOR_GITHUB_API", "MOOR_GITHUB_WEB"] {
        if !preset.iter().any(|(p, _)| p == var) {
            pairs.push((var, "http://127.0.0.1:9".to_string()));
        }
    }
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

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

fn class_of(envelope: Option<&Value>, exit: i32, stderr: &str) -> ExitClass {
    match envelope
        .and_then(|j| j.get("class"))
        .and_then(Value::as_str)
    {
        Some("ok") => ExitClass::Ok,
        Some("failed") => ExitClass::Failed,
        Some("usage") => ExitClass::Usage,
        Some("refused") => ExitClass::Refused,
        other => panic!(
            "new binary gave no usable envelope class ({other:?}): exit {exit}, stderr: {stderr}"
        ),
    }
}

/// Fail the case when the envelope reports an unimplemented command.
fn reject_not_implemented(envelope: Option<&Value>, args: &[String]) {
    let Some(err) = envelope.and_then(|j| j.get("error")) else {
        return;
    };
    let kind = err.get("kind").and_then(Value::as_str);
    let msg = err.get("message").and_then(Value::as_str).unwrap_or("");
    if kind == Some("internal") && msg.contains("not implemented") {
        panic!("not implemented on new: {}", args.join(" "));
    }
}

/// Append the envelope error message and hints so `stderr.contains(..)` works.
fn with_error_text(envelope: Option<&Value>, mut stderr: String) -> String {
    let Some(err) = envelope
        .and_then(|j| j.get("error"))
        .filter(|e| e.is_object())
    else {
        return stderr;
    };
    if let Some(m) = err.get("message").and_then(Value::as_str) {
        stderr.push_str(&format!("\n{m}"));
    }
    for h in err
        .get("hints")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(h) = h.as_str() {
            stderr.push_str(&format!("\n{h}"));
        }
    }
    stderr
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
    reject_not_implemented(envelope.as_ref(), args);
    let class = class_of(envelope.as_ref(), exit, &stderr);
    let value = envelope
        .as_ref()
        .and_then(|j| j.get("value"))
        .filter(|v| !v.is_null())
        .cloned();
    let mut stderr = with_error_text(envelope.as_ref(), stderr);
    let warnings = warnings_of(envelope.as_ref());
    for m in &warnings {
        stderr.push_str(&format!("\n{m}"));
    }
    // Old CLI prints the command's value on stdout; mirror that, not the envelope.
    let stdout = match (&value, &envelope) {
        (Some(v), _) => v.to_string(),
        (None, Some(_)) => String::new(),
        (None, None) => stdout,
    };
    Outcome {
        exit,
        class,
        value,
        warnings,
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

/// A seam the new binary does not map yet: fail the case, never pass quietly.
pub(crate) fn unmapped(what: &str) -> ! {
    panic!("not mapped yet: {what}")
}
