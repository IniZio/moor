//! Command-line surface: argument parsing, rendering and exit codes.

mod args;
mod args_admin;
mod args_probe;
mod caller;
mod exit;
mod render_json;
mod render_text;
#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::path::PathBuf;

use clap::error::ErrorKind as ClapKind;
use moor_core::{ErrorKind, ErrorView, Request, Response, RunReq};

use args::Action;
pub(crate) use caller::caller_repo;
pub use caller::{resolve, Resolved};

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
    /// Bring the store's schema to this build's latest migration.
    StoreMigrate {
        /// Global options.
        opts: GlobalOpts,
    },
    /// Copy the `SQLite` store into an empty Postgres database.
    StoreCopy {
        /// Global options.
        opts: GlobalOpts,
        /// The Postgres URL to copy into.
        to: String,
    },
    /// Bridge stdin/stdout to the controller socket.
    ControllerAttach {
        /// Global options.
        opts: GlobalOpts,
        /// Controller socket override.
        socket: Option<std::path::PathBuf>,
    },
    /// Bridge stdin/stdout to a pool lease's sshd through the links broker.
    LinkDial {
        /// Global options.
        opts: GlobalOpts,
        /// The pool host's machine id.
        machine_id: i64,
        /// The lease.
        slug: String,
    },
    /// Check the setup and print one line per check.
    Doctor {
        /// Global options.
        opts: GlobalOpts,
        /// Public URL of the gate, to check it answers.
        public_url: Option<String>,
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
    /// Run a command in a Box with this process's stdio streamed to it.
    Exec {
        /// Global options.
        opts: GlobalOpts,
        /// The command and its Box.
        req: moor_core::ExecReq,
        /// Forward stdin even from a terminal.
        interactive: bool,
    },
    /// Read a secret from stdin and send the request it completes.
    Stdin {
        /// Global options.
        opts: GlobalOpts,
        /// The request stdin completes.
        then: StdinRequest,
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
        Ok((opts, Action::StoreMigrate)) => Parsed::StoreMigrate { opts },
        Ok((opts, Action::StoreCopy { to })) => Parsed::StoreCopy { opts, to },
        Ok((opts, Action::Stdin(then))) => Parsed::Stdin { opts, then },
        Ok((opts, Action::Exec(req, interactive))) => Parsed::Exec {
            opts,
            req,
            interactive,
        },
        Ok((opts, Action::LinkDial { machine_id, slug })) => Parsed::LinkDial {
            opts,
            machine_id,
            slug,
        },
        Ok((opts, Action::Doctor { public_url })) => Parsed::Doctor { opts, public_url },
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
            detail: None,
        };
        let opts = GlobalOpts {
            json: true,
            ..GlobalOpts::default()
        };
        render(&Response::err("call", view), &opts)
    })
}

/// A request whose secret is read from stdin, so it never shows in argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdinRequest {
    /// `secret-set` of this name.
    SecretSet(moor_core::user::SecretName),
    /// `webhook-setup-finish` of this webhook; empty stdin gives no header value.
    WebhookFinish(i64),
}

impl StdinRequest {
    /// The request, with `stdin` less trailing line breaks as its secret.
    #[must_use]
    pub fn request(self, mut stdin: Vec<u8>) -> Request {
        use moor_core::request::{SecretSetReq, SecretValue, WebhookSetupFinishReq};
        while stdin.last().is_some_and(|b| matches!(b, b'\n' | b'\r')) {
            stdin.pop();
        }
        match self {
            Self::SecretSet(name) => Request::SecretSet(SecretSetReq {
                identity: None,
                name,
                value: SecretValue::new(stdin),
            }),
            Self::WebhookFinish(id) => Request::WebhookSetupFinish(WebhookSetupFinishReq {
                identity: None,
                id,
                header_value: (!stdin.is_empty()).then(|| SecretValue::new(stdin)),
            }),
        }
    }

    /// The request's kind, for an error before it is built.
    #[must_use]
    pub const fn cmd(&self) -> &'static str {
        match self {
            Self::SecretSet(_) => "secret-set",
            Self::WebhookFinish(_) => "webhook-setup-finish",
        }
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

/// A refused value's message without the value, which may be a URL that carries a token.
fn unquoted_value_error(err: &clap::Error) -> Option<String> {
    if err.kind() != ClapKind::ValueValidation {
        return None;
    }
    let why = std::error::Error::source(err)?;
    Some(err.get(clap::error::ContextKind::InvalidArg).map_or_else(
        || format!("invalid value: {why}"),
        |arg| format!("invalid value for '{arg}': {why}"),
    ))
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
                detail: None,
            };
            render(&Response::err(cmd, view), opts)
        }
        _ => {
            let mut lines = text.lines();
            let first = lines.next().unwrap_or_default();
            let message = unquoted_value_error(err)
                .unwrap_or_else(|| first.strip_prefix("error: ").unwrap_or(first).to_owned());
            let hints = lines
                .filter(|l| !l.trim().is_empty())
                .map(str::to_owned)
                .collect();
            let view = ErrorView {
                kind: ErrorKind::Usage,
                message,
                hints,
                context: Vec::new(),
                detail: None,
            };
            render(&Response::err(cmd, view), opts)
        }
    }
}
