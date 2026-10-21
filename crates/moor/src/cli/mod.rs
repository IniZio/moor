//! Command-line surface: argument parsing, rendering and exit codes.

mod args;
mod args_admin;
mod args_probe;
mod exit;
mod render_json;
mod render_text;
#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::error::ErrorKind as ClapKind;
use moor_core::request::InReq;
use moor_core::{CpReq, EnvReq, ErrorKind, ErrorView, Request, Response, RunReq, StartReq};

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
    /// Run the proxy supervisor for a Box.
    ProxyServe {
        /// Global options.
        opts: GlobalOpts,
        /// Box to serve.
        box_id: moor_core::BoxId,
    },
    /// Run the controller daemon from a config file.
    ControllerServe {
        /// Global options.
        opts: GlobalOpts,
        /// Controller config file.
        config: std::path::PathBuf,
    },
    /// Bridge stdin/stdout to the controller socket.
    ControllerAttach {
        /// Global options.
        opts: GlobalOpts,
        /// Controller socket override.
        socket: Option<std::path::PathBuf>,
    },
    /// Print the event stream as JSON lines.
    Events {
        /// Global options.
        opts: GlobalOpts,
        /// Only events after this id.
        since: i64,
        /// Which events to print.
        filter: moor_core::controller::WatchFilter,
        /// Keep streaming.
        follow: bool,
    },
    /// Read a secret value from stdin and store it as this name with `secret-set`.
    SecretSetStdin {
        /// Global options.
        opts: GlobalOpts,
        /// The secret's name.
        name: moor_core::user::SecretName,
    },
    /// Nothing to run; print this.
    Done(Output),
}

/// Parses argv (`args[0]` is the program name).
#[must_use]
pub fn parse(args: Vec<OsString>) -> Parsed {
    let (raw, cmd) = args_probe::recover(&args);
    match args::parse(args) {
        Ok((opts, Action::Request(request))) => Parsed::Request(Invocation { opts, request }),
        Ok((opts, Action::CallStdin)) => Parsed::CallStdin(GlobalOpts { json: true, ..opts }),
        Ok((opts, Action::ProxyServe(box_id))) => Parsed::ProxyServe { opts, box_id },
        Ok((opts, Action::ControllerServe(config))) => Parsed::ControllerServe { opts, config },
        Ok((opts, Action::ControllerAttach(socket))) => Parsed::ControllerAttach { opts, socket },
        Ok((opts, Action::SecretSetStdin(name))) => Parsed::SecretSetStdin { opts, name },
        Ok((
            opts,
            Action::Events {
                since,
                filter,
                follow,
            },
        )) => Parsed::Events {
            opts,
            since,
            filter,
            follow,
        },
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

/// The `secret-set` request for `name` with `stdin` as its value, less trailing line breaks.
#[must_use]
pub fn secret_set(name: moor_core::user::SecretName, mut stdin: Vec<u8>) -> Request {
    while stdin.last().is_some_and(|b| matches!(b, b'\n' | b'\r')) {
        stdin.pop();
    }
    Request::SecretSet(moor_core::request::SecretSetReq {
        identity: None,
        name,
        value: moor_core::request::SecretValue::new(stdin),
    })
}

/// `request` resolved against `cwd`, the caller's directory.
///
/// The controller never reads a directory of its own. A `start`, `start-needs` or `env` with no
/// repo runs `in` `cwd`, and a relative `--worktree`, `cp` destination or `env` repo becomes
/// absolute.
#[must_use]
pub fn in_cwd(request: Request, cwd: &Path) -> Request {
    let abs = |p: String| cwd.join(p).display().to_string();
    let inside = |request: Request| {
        Request::In(InReq {
            dir: cwd.display().to_string(),
            request: Box::new(request),
        })
    };
    match request {
        Request::Start(r) => {
            let no_repo = r.repo.is_none();
            let r = Request::Start(StartReq {
                worktree: r.worktree.map(abs),
                ..r
            });
            if no_repo {
                inside(r)
            } else {
                r
            }
        }
        Request::StartNeeds(r) if r.repo.is_none() => inside(Request::StartNeeds(r)),
        Request::Env(EnvReq { repo: None }) => inside(Request::Env(EnvReq { repo: None })),
        Request::Env(EnvReq { repo: Some(r) }) => Request::Env(EnvReq { repo: Some(abs(r)) }),
        Request::Cp(r) => Request::Cp(CpReq {
            dest: abs(r.dest),
            ..r
        }),
        other => other,
    }
}

/// Marks a text-mode `run` request to stream program output to the terminal.
///
/// With `--json` the output stays in the run log so stdout carries only the
/// envelope. Other requests pass through unchanged.
#[must_use]
pub fn inherit_run_output(request: Request, opts: &GlobalOpts) -> Request {
    match request {
        Request::Run(run) if !opts.json => Request::Run(RunReq {
            inherit_output: true,
            ..run
        }),
        other => other,
    }
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
        ClapKind::DisplayHelp | ClapKind::DisplayVersion if opts.json => render(
            &Response::ok(cmd, Some(serde_json::Value::String(text))),
            opts,
        ),
        ClapKind::DisplayHelp | ClapKind::DisplayVersion => Output {
            stdout: text,
            stderr: String::new(),
            code: 0,
        },
        ClapKind::DisplayHelpOnMissingArgumentOrSubcommand if !opts.json => Output {
            stdout: String::new(),
            stderr: text,
            code: 2,
        },
        ClapKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            let view = ErrorView {
                kind: ErrorKind::Usage,
                message: "missing subcommand".to_owned(),
                hints: text.lines().map(str::to_owned).collect(),
                context: Vec::new(),
            };
            render(&Response::err(cmd, view), opts)
        }
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
