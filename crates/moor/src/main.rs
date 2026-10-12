//! `moor` binary: the only place that writes to stdout and stderr.
#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::process::ExitCode;

use moor::cli::{self, GlobalOpts, Invocation, Output, Parsed};
use moor::env::Env;
use moor::moor_core::Request;
use moor::Moor;

/// Parse argv, run the request, print the rendered output, return its exit code.
fn main() -> ExitCode {
    let env = Env::capture();
    let output = match cli::parse(Env::args()) {
        Parsed::Done(out) => out,
        Parsed::Request(inv) => run(env, inv),
        Parsed::CallStdin(opts) => run_call(env, opts),
    };
    emit(&output)
}

/// Read a request from stdin and run it; always JSON output.
fn run_call(env: Env, opts: GlobalOpts) -> Output {
    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        let resp = moor::error::CommandError::new(moor::moor_core::ErrorKind::Failed, e)
            .into_response("call", vec![]);
        return cli::render(&resp, &opts);
    }
    match cli::parse_call(&input) {
        Ok(request) => run(env, Invocation { opts, request }),
        Err(out) => out,
    }
}

/// Build the engine from the environment, run one request, render the response.
fn run(env: Env, inv: Invocation) -> Output {
    let Invocation { opts, request } = inv;
    let env = match &opts.state_dir {
        Some(dir) => env.with_state_dir(dir),
        None => env,
    };
    render_call(&env, request, &opts)
}

/// Run `request` against a fresh engine and render the response with `opts`.
fn render_call(env: &Env, request: Request, opts: &GlobalOpts) -> Output {
    let cmd = request.cmd();
    let resp = if matches!(request, Request::Version {}) {
        Moor::version()
    } else {
        match Moor::from_env(env) {
            Ok(moor) => moor.call(request),
            Err(e) => e.into_response(cmd, vec![]),
        }
    };
    cli::render(&resp, opts)
}

/// Write stdout then stderr (write errors ignored) and map the code.
fn emit(out: &Output) -> ExitCode {
    let _ = std::io::stdout().write_all(out.stdout.as_bytes());
    let _ = std::io::stderr().write_all(out.stderr.as_bytes());
    ExitCode::from(out.code)
}
