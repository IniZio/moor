//! `moor` binary: the only place that opens stdout and stderr. It writes rendered
//! output itself and hands stderr to the Proxy processes and the controller
//! daemon, which stream lines live.
#![forbid(unsafe_code)]

use std::io::{IsTerminal, Read, Write};
use std::process::ExitCode;

use moor::cli::{self, GlobalOpts, Invocation, Output, Parsed};
use moor::client::ExecIo;
use moor::env::Env;
use moor::moor_core::controller::WatchFilter;
use moor::moor_core::seam::proc::Pipes;
use moor::moor_core::view::ExecView;
use moor::moor_core::{ExecReq, Request, Response};
use moor::Moor;

/// Runs `moor front-shim`, the controller's wrapper around each front, `moor front-run`, the
/// wrapper of a front the controller does not start, `moor proxy serve-fronts`, the front
/// Proxy the controller runs, or `moor proxy serve-egress`, the shared egress Proxy; none is a
/// user command.
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
        if proxy == "proxy" && serve == "serve-egress" {
            return Some(ExitCode::from(
                moor::serve_egress_proxy(args).unwrap_or_else(|e| {
                    let _ = writeln!(std::io::stderr(), "moor egress proxy: {e}");
                    1
                }),
            ));
        }
    }
    let [_, verb, args @ ..] = args else {
        return None;
    };
    let run = match verb.to_str() {
        Some("front-shim") => moor::controller::front_shim::main,
        Some("front-run") => moor::controller::front_shim::run,
        _ => return None,
    };
    Some(ExitCode::from(run(args).unwrap_or_else(|e| {
        let _ = writeln!(std::io::stderr(), "{}: {e}", verb.to_string_lossy());
        125
    })))
}

/// Writes `line` to stdout and flushes, so a reader of `moor listen` sees it at once.
fn say(line: &str) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}")?;
    out.flush()
}

/// Parse argv, run the request, print the rendered output, return its exit code.
fn main() -> ExitCode {
    let args = Env::args();
    if let Some(code) = front_shim(&args).or_else(|| moor::mcp_relay::run(&args)) {
        return code;
    }
    if let Some(out) = moor::sign_in::run(&args).or_else(|| moor::listen_cmd::run(&args, &mut say))
    {
        return emit(&out);
    }
    let env = Env::capture();
    let output = match cli::parse(args) {
        Parsed::Done(out) => out,
        Parsed::Request(inv) => run(env, inv),
        Parsed::CallStdin(opts) => run_call(env, opts),
        Parsed::Stdin { opts, then } => run_stdin(env, opts, then),
        Parsed::ControllerServe { opts, config } => {
            let env = scoped(env, &opts);
            return ExitCode::from(moor::controller::serve(&env, &config, std::io::stderr()));
        }
        Parsed::StoreMigrate { opts } => return migrate_store(&scoped(env, &opts)),
        Parsed::StoreCopy { opts, to } => return copy_store(&scoped(env, &opts), &to),
        Parsed::ControllerAttach { opts, socket } => {
            let env = scoped(env, &opts);
            return ExitCode::from(moor::controller::attach::attach(
                &env,
                socket.as_deref(),
                std::io::stdin(),
                std::io::stdout(),
                std::io::stderr(),
            ));
        }
        Parsed::Exec {
            opts,
            req,
            interactive: i,
        } => return run_exec(&scoped(env, &opts), req, i),
        Parsed::LinkDial {
            opts,
            runner,
            machine_id,
            slug,
        } => {
            let env = scoped(env, &opts);
            return ExitCode::from(moor::link_dial::on_stdio(
                &env,
                &runner,
                (machine_id, &slug),
            ));
        }
        Parsed::Doctor { opts, public_url } => doctor(env, &opts, public_url.as_deref()),
        Parsed::Events {
            opts,
            since,
            filter,
            follow,
        } => {
            let env = scoped(env, &opts);
            return run_events(&env, since, &filter, follow);
        }
    };
    emit(&output)
}

/// Runs `moor store migrate`: opens the store, which migrates it, and exits.
fn migrate_store(env: &Env) -> ExitCode {
    match moor::migrate_store(env) {
        Ok(line) => {
            let _ = say(&line);
            ExitCode::SUCCESS
        }
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "moor store migrate: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Runs `moor store copy`: copies the store into Postgres and exits.
fn copy_store(env: &Env, to: &str) -> ExitCode {
    match moor::copy_store(env, to) {
        Ok(report) => {
            let _ = say(&report);
            ExitCode::SUCCESS
        }
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "moor store copy: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `env` with the state dir `opts` names, if any.
fn scoped(env: Env, opts: &cli::GlobalOpts) -> Env {
    match &opts.state_dir {
        Some(dir) => env.with_state_dir(dir),
        None => env,
    }
}

/// The `doctor` report for this host.
fn doctor(env: Env, opts: &GlobalOpts, public_url: Option<&str>) -> Output {
    moor::doctor::run(&scoped(env, opts), opts, public_url)
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

/// Runs `moor exec` with this process's stdio. The command's exit code is ours; moor's own
/// failure, or a command ended without one, exits 255 as ssh does.
fn run_exec(env: &Env, req: ExecReq, interactive: bool) -> ExitCode {
    let stdin = std::io::stdin();
    let stdin =
        (interactive || !stdin.is_terminal()).then(|| Box::new(stdin) as Box<dyn Read + Send>);
    let (mut stdout, mut stderr) = (std::io::stdout(), std::io::stderr());
    let resp = if let Some(target) = moor::client::configured_target(env) {
        let io = ExecIo {
            stdin,
            stdout: &mut stdout,
            stderr: &mut stderr,
        };
        moor::client::exec(env, &target, req, io)
            .unwrap_or_else(|e| moor::client::failure_response("exec", &e))
    } else {
        let pipes = Pipes {
            stdin: stdin.unwrap_or_else(|| Box::new(std::io::empty())),
            stdout: &mut stdout,
            stderr: &mut stderr,
            stop: &|| false,
        };
        match Moor::from_env(env).and_then(|moor| moor.exec(&req, pipes)) {
            Ok(view) => Response::ok("exec", serde_json::to_value(view).ok()),
            Err(e) => e.into_response("exec", vec![]),
        }
    };
    let exit = resp
        .value
        .clone()
        .and_then(|v| serde_json::from_value::<ExecView>(v).ok());
    match exit.and_then(|v| v.exit) {
        Some(code) if resp.error.is_none() => ExitCode::from(u8::try_from(code).unwrap_or(255)),
        _ => {
            let text = cli::render(&resp, &GlobalOpts::default());
            let _ = std::io::stderr().write_all(text.stderr.as_bytes());
            ExitCode::from(255)
        }
    }
}

/// Read the secret `then` needs from stdin, then run the request it completes.
fn run_stdin(env: Env, opts: GlobalOpts, then: cli::StdinRequest) -> Output {
    let mut value = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut value) {
        let resp = moor::error::CommandError::new(moor::moor_core::ErrorKind::Failed, e)
            .into_response(then.cmd(), vec![]);
        return cli::render(&resp, &opts);
    }
    let request = then.request(value);
    run(env, Invocation { opts, request })
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
        Some(target) => moor::client::remote::run(&env, &target, &request, &opts),
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
