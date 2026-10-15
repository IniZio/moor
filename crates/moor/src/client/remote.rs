//! Client side of controller mode: send the request to the daemon, stream its
//! progress, and run interactive commands locally.

use std::io::Write;
use std::path::Path;
use std::process::Command;

use moor_core::seam::proc::Tool;
use moor_core::{ErrorKind, ErrorView, Request, Response};

use crate::cli::{render, GlobalOpts, Output};
use crate::env::Env;

/// Sends `request` to the controller at `socket` and renders its answer.
///
/// Text mode prints progress frames to stderr as they arrive; `--json` keeps
/// stderr quiet so stdout stays one envelope.
#[must_use]
pub fn run(env: &Env, socket: &Path, request: Request, opts: &GlobalOpts) -> Output {
    let interactive = matches!(request, Request::Open(_));
    let request = match request {
        Request::Open(t) => Request::Connect(t),
        other => other,
    };
    let result = super::call(socket, &request, |msg| {
        if !opts.json {
            let mut err = std::io::stderr();
            let _ = writeln!(err, "moor: {msg}");
            let _ = err.flush();
        }
    });
    let resp = match result {
        Ok(resp) if interactive => attach(env, resp),
        Ok(resp) => resp,
        Err(e) => super::failure_response(request.cmd(), &e),
    };
    render(&resp, opts)
}

/// Runs the connect command the controller returned and reports it as `open`.
fn attach(env: &Env, mut resp: Response) -> Response {
    "open".clone_into(&mut resp.cmd);
    if resp.error.is_some() {
        return resp;
    }
    let Some(value) = resp.value.clone() else {
        return failed("controller returned no connect command");
    };
    let tool = value.get("tool").and_then(|v| v.as_str()).unwrap_or("");
    let args: Vec<String> = value
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if tool != Tool::Herdr.program() {
        return failed(&format!("controller asked to run unknown tool `{tool}`"));
    }
    let program = env.get(Tool::Herdr.bin_var()).unwrap_or(tool).to_owned();
    let shown = format!("{program} {}", args.join(" "));
    match Command::new(&program).args(&args).status() {
        Ok(status) if status.success() => resp,
        Ok(status) => failed(&format!("`{}` exited with {status}", shown.trim_end())),
        Err(e) => failed(&format!("cannot run `{}`: {e}", shown.trim_end())),
    }
}

#[cfg(test)]
mod tests;

/// A failed `open` envelope.
fn failed(message: &str) -> Response {
    Response::err(
        "open",
        ErrorView {
            kind: ErrorKind::Failed,
            message: message.to_owned(),
            hints: Vec::new(),
            context: Vec::new(),
        },
    )
}
