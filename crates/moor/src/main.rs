//! `moor` binary: the only place that opens stdout and stderr. It writes rendered
//! output itself and hands stderr to the proxy supervisor and the controller
//! daemon, which stream lines live.
#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::process::ExitCode;

use moor::cli::{self, GlobalOpts, Invocation, Output, Parsed};
use moor::env::Env;
use moor::moor_core::controller::WatchFilter;
use moor::moor_core::{Request, Response};
use moor::Moor;

/// Runs `moor front-shim`, the controller's wrapper around each front, or `moor proxy
/// serve-fronts`, the front Proxy the controller runs; neither is a user command.
fn front_shim(args: &[std::ffi::OsString]) -> Option<ExitCode> {
    if let [_, proxy, serve, args @ ..] = args {
        if proxy == "proxy" && serve == "serve-fronts" {
            return Some(ExitCode::from(
                moor::serve_front_proxy(args).unwrap_or_else(|e| {
                    let _ = writeln!(std::io::stderr(), "moor front proxy: {e}");
                    1
                }),
            ));
        }
    }
    let [_, verb, args @ ..] = args else {
        return None;
    };
    (verb == "front-shim").then(|| {
        ExitCode::from(
            moor::controller::front_shim::main(args).unwrap_or_else(|e| {
                let _ = writeln!(std::io::stderr(), "front-shim: {e}");
                125
            }),
        )
    })
}

/// Parse argv, run the request, print the rendered output, return its exit code.
fn main() -> ExitCode {
    let args = Env::args();
    if let Some(code) = front_shim(&args) {
        return code;
    }
    let env = Env::capture();
    let output = match cli::parse(args) {
        Parsed::Done(out) => out,
        Parsed::Request(inv) => run(env, inv),
        Parsed::CallStdin(opts) => run_call(env, opts),
        Parsed::SecretSetStdin { opts, name } => {
            let mut value = Vec::new();
            if let Err(e) = std::io::stdin().read_to_end(&mut value) {
                let resp = moor::error::CommandError::new(moor::moor_core::ErrorKind::Failed, e)
                    .into_response("secret-set", vec![]);
                return emit(&cli::render(&resp, &opts));
            }
            let request = cli::secret_set(name, value);
            run(env, Invocation { opts, request })
        }
        Parsed::ProxyServe { opts, box_id } => {
            let env = match &opts.state_dir {
                Some(dir) => env.with_state_dir(dir),
                None => env,
            };
            return ExitCode::from(moor::serve_proxy(&env, box_id, std::io::stderr()));
        }
        Parsed::ControllerServe { opts, config } => {
            let env = match &opts.state_dir {
                Some(dir) => env.with_state_dir(dir),
                None => env,
            };
            return ExitCode::from(moor::controller::serve(&env, &config, std::io::stderr()));
        }
        Parsed::ControllerAttach { opts, socket } => {
            let env = match &opts.state_dir {
                Some(dir) => env.with_state_dir(dir),
                None => env,
            };
            return ExitCode::from(moor::controller::attach::attach(
                &env,
                socket.as_deref(),
                std::io::stdin(),
                std::io::stdout(),
                std::io::stderr(),
            ));
        }
        Parsed::Events {
            opts,
            since,
            filter,
            follow,
        } => {
            let env = match &opts.state_dir {
                Some(dir) => env.with_state_dir(dir),
                None => env,
            };
            return run_events(&env, since, &filter, follow);
        }
    };
    emit(&output)
}

/// Stream events as JSON lines from the configured controller, else the local store.
fn run_events(env: &Env, since: i64, filter: &WatchFilter, follow: bool) -> ExitCode {
    let mut out = std::io::stdout().lock();
    let result = match moor::client::configured_target(env) {
        Some(target) => moor::client::watch(env, &target, since, filter, follow, &mut out)
            .map_err(|e| e.to_string()),
        None => moor::controller::watch_local(env, since, filter, follow, &mut out),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "{e}");
            ExitCode::FAILURE
        }
    }
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
    let cmd = request.cmd();
    // The client's own directory; an unreadable one leaves relative paths for the controller
    // to refuse.
    let resolved = match std::env::current_dir() {
        Ok(cwd) => cli::resolve(request, &cwd, &Moor::host_checkout(&env)),
        Err(_) => Ok(cli::Resolved {
            request,
            warning: None,
        }),
    };
    let cli::Resolved { request, warning } = match resolved {
        Ok(r) => r,
        Err(why) => {
            let resp = moor::error::CommandError::new(moor::moor_core::ErrorKind::Usage, why)
                .into_response(cmd, vec![]);
            return cli::render(&resp, &opts);
        }
    };
    let mut resp = match moor::client::configured_target(&env) {
        Some(target) => moor::client::remote::run(&env, &target, request, &opts),
        None => call(&env, cli::inherit_run_output(request, &opts)),
    };
    resp.warnings.extend(warning);
    cli::render(&resp, &opts)
}

/// Run `request` against a fresh engine.
fn call(env: &Env, request: Request) -> Response {
    if matches!(request, Request::Version {}) {
        return Moor::version();
    }
    match Moor::from_env(env) {
        Ok(moor) => moor.call(request),
        Err(e) => e.into_response(request.cmd(), vec![]),
    }
}

/// Write stdout then stderr (write errors ignored) and map the code.
fn emit(out: &Output) -> ExitCode {
    let _ = std::io::stdout().write_all(out.stdout.as_bytes());
    let _ = std::io::stderr().write_all(out.stderr.as_bytes());
    ExitCode::from(out.code)
}
