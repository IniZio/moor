//! Command-line surface: argument parsing, rendering and exit codes.

mod args;
mod args_admin;
mod exit;
mod render_json;
mod render_text;
#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::path::PathBuf;

use clap::error::ErrorKind as ClapKind;
use moor_core::{ErrorKind, ErrorView, Request, Response};

use crate::error::CommandError;

use args::Action;

/// Options shared by every command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalOpts {
    /// Print the JSON envelope instead of text.
    pub json: bool,
    /// Suppress warnings in text output.
    pub quiet: bool,
    /// Override of the state directory.
    pub state_dir: Option<PathBuf>,
}

/// A parsed request with its global options.
#[derive(Debug)]
pub struct Invocation {
    /// Global options.
    pub opts: GlobalOpts,
    /// The request to run.
    pub request: Request,
}

/// Rendered process output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// Bytes for stdout.
    pub stdout: String,
    /// Bytes for stderr.
    pub stderr: String,
    /// Process exit code.
    pub code: u8,
}

/// Result of parsing argv.
#[derive(Debug)]
pub enum Parsed {
    /// Run this request.
    Request(Invocation),
    /// Read a request from stdin.
    CallStdin(GlobalOpts),
    /// Nothing to run; print this.
    Done(Output),
}

/// Parses argv (`args[0]` is the program name).
#[must_use]
pub fn parse(args: Vec<OsString>) -> Parsed {
    let raw = raw_opts(&args);
    let cmd = detect_cmd(&args);
    match args::parse(args) {
        Ok((opts, Action::Request(request))) => Parsed::Request(Invocation { opts, request }),
        Ok((opts, Action::CallStdin)) => Parsed::CallStdin(GlobalOpts { json: true, ..opts }),
        Ok((opts, Action::ProxyServe)) => {
            let resp =
                CommandError::not_implemented("proxy-serve").into_response("proxy-serve", vec![]);
            Parsed::Done(render(&resp, &opts))
        }
        Err(err) => Parsed::Done(clap_output(&err, cmd, &raw)),
    }
}

/// Parses a request from the `call` stdin payload.
pub fn parse_call(stdin: &str) -> Result<Request, Output> {
    serde_json::from_str::<Request>(stdin).map_err(|e| {
        let view = ErrorView {
            kind: ErrorKind::Usage,
            message: e.to_string(),
            hints: Vec::new(),
            context: Vec::new(),
        };
        let opts = GlobalOpts {
            json: true,
            ..GlobalOpts::default()
        };
        render(&Response::err("call", view), &opts)
    })
}

/// Renders a response for the given options.
#[must_use]
pub fn render(resp: &Response, opts: &GlobalOpts) -> Output {
    let code = exit::code(resp);
    if opts.json {
        Output {
            stdout: render_json::render(resp),
            stderr: String::new(),
            code,
        }
    } else {
        let (stdout, stderr) = render_text::render(resp, opts.quiet);
        Output {
            stdout,
            stderr,
            code,
        }
    }
}

/// Maps a clap error to help output or a usage envelope.
fn clap_output(err: &clap::Error, cmd: String, opts: &GlobalOpts) -> Output {
    let text = err.render().to_string();
    match err.kind() {
        ClapKind::DisplayHelp | ClapKind::DisplayVersion => Output {
            stdout: text,
            stderr: String::new(),
            code: 0,
        },
        ClapKind::DisplayHelpOnMissingArgumentOrSubcommand => Output {
            stdout: String::new(),
            stderr: text,
            code: 2,
        },
        _ => {
            let mut lines = text.lines();
            let first = lines.next().unwrap_or_default();
            let message = first.strip_prefix("error: ").unwrap_or(first).to_owned();
            let hints = lines
                .filter(|l| !l.trim().is_empty())
                .map(str::to_owned)
                .collect();
            let view = ErrorView {
                kind: ErrorKind::Usage,
                message,
                hints,
                context: Vec::new(),
            };
            render(&Response::err(cmd, view), opts)
        }
    }
}

/// Reads `--json`, `--quiet` and `--state-dir` straight from argv.
fn raw_opts(args: &[OsString]) -> GlobalOpts {
    let mut opts = GlobalOpts::default();
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        match a.to_str() {
            Some("--") => break,
            Some("--json") => opts.json = true,
            Some("--quiet") => opts.quiet = true,
            Some("--state-dir") => opts.state_dir = it.next().map(PathBuf::from),
            _ => {}
        }
    }
    opts
}

/// First non-flag argument that is not the value of `--state-dir`.
fn detect_cmd(args: &[OsString]) -> String {
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        let s = a.to_string_lossy();
        if s == "--state-dir" {
            it.next();
        } else if !s.starts_with('-') {
            return s.into_owned();
        }
    }
    String::new()
}
