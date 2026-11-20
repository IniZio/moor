//! `moor-machine-agent`: pairs this computer with a moor server and keeps it linked.

use std::io::Write;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use moor_machine_agent::pair::Token;
use moor_machine_agent::service::Manager;
use moor_machine_agent::up::{self, down, Runtime, Up};
use moor_machine_agent::{
    parse_age, run_hosting, state_dir, until_stopped, update, Ended, Host, Images, Machine,
};

/// The kind of Boxes a machine makes unless it is told otherwise.
const DOCKER: &str = "docker";

/// Command line.
#[derive(Parser)]
#[command(name = "moor-machine-agent", version = option_env!("MOOR_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")))]
struct Args {
    /// What to do.
    #[command(subcommand)]
    command: Command,
}

/// Subcommands.
#[derive(Subcommand)]
enum Command {
    /// Pairs this machine with its owner, checks that Docker works, and runs the agent as a
    /// user service that starts at login; rerun it to apply new flags. The agent makes each Box
    /// of two Docker containers, and the work place is privileged.
    Up(UpFlags),
    /// Stops and removes the user service; the machine stays paired.
    Down,
    /// Keeps this paired machine linked to its server until stopped; the user service runs it.
    Run {
        /// Run `exec` scripts the server sends and provide no Boxes; without it the machine
        /// provides Docker Boxes and refuses `exec`.
        #[arg(long)]
        allow_exec: bool,
        /// How this machine makes Boxes; only `docker` is released.
        #[arg(long, value_name = "KIND", conflicts_with = "allow_exec", hide = true)]
        boxes: Option<String>,
        /// The images Docker Boxes are made from.
        #[command(flatten)]
        images: ImageFlags,
        /// When the server needs another release, install it and restart in place.
        #[arg(long)]
        auto_update: bool,
    },
    /// Installs a release of this agent in place of the running one.
    Update {
        /// The release, such as `1.2.3`.
        #[arg(long)]
        version: String,
    },
}

/// What `up` takes.
#[derive(clap::Args)]
struct UpFlags {
    /// The moor server, such as `https://moor.example.com`.
    #[arg(long, env = "MOOR_SERVER")]
    server: String,
    /// A one-time token from the portal's Machines page; adds this machine with no browser step.
    #[arg(
        long,
        env = "MOOR_PAIR_TOKEN",
        hide_env_values = true,
        value_name = "TOKEN"
    )]
    token: Option<String>,
    /// The machine's name, unique among your machines; this host's name by default.
    #[arg(long)]
    name: Option<String>,
    /// Labels a Box can ask for, comma separated.
    #[arg(long, value_delimiter = ',')]
    labels: Vec<String>,
    /// Host no Boxes; run the `exec` scripts the server sends for Boxes that attach this machine.
    #[arg(long)]
    allow_exec: bool,
    /// How this machine makes Boxes; only `docker` is released.
    #[arg(long, value_name = "KIND", conflicts_with = "allow_exec", hide = true)]
    boxes: Option<String>,
    /// The images Docker Boxes are made from.
    #[command(flatten)]
    images: ImageFlags,
}

/// Brings this machine to linked as `flags` say.
fn up_here(flags: UpFlags) -> ExitCode {
    let runtime = if flags.allow_exec {
        Runtime::Exec
    } else {
        Runtime::Boxes {
            kind: flags.boxes.unwrap_or_else(|| DOCKER.to_owned()),
            agent_image: flags.images.agent_image,
            work_image: flags.images.work_image,
            cache_max_age: flags.images.cache_max_age,
        }
    };
    let home = Machine::from_env().home;
    let req = Up {
        server: flags.server,
        name: flags.name.unwrap_or_else(host_name),
        labels: flags.labels,
        runtime,
        token: flags.token.map(Token::new),
    };
    done(up::up(
        &req,
        &state_dir(),
        &Manager::here(&home),
        &mut |line| say(line),
    ))
}

fn main() -> ExitCode {
    let ended = match Args::parse().command {
        Command::Up(flags) => return up_here(flags),
        Command::Down => {
            return done(down(
                &Manager::here(&Machine::from_env().home),
                &state_dir(),
            ));
        }
        Command::Run {
            allow_exec,
            boxes,
            images,
            auto_update,
        } => run_here((allow_exec, boxes, images), auto_update),
        Command::Update { version } => return update_here(&version),
    };
    match ended {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            say(&e);
            ExitCode::FAILURE
        }
    }
}

/// The images a Docker Box is made from; each left out is this release's own.
#[derive(clap::Args, Debug)]
struct ImageFlags {
    /// The agent place's image, such as one built from `images/agent` for development.
    #[arg(long, env = "MOOR_AGENT_IMAGE", value_name = "IMAGE")]
    agent_image: Option<String>,
    /// The work place's image, such as one built from `images/work` for development.
    #[arg(long, env = "MOOR_WORK_IMAGE", value_name = "IMAGE")]
    work_image: Option<String>,
    /// How long a repo's cache volumes may sit unused before moor deletes them, such as `30d`,
    /// `12h` or `20s`; a bare number is days and `0` keeps them for good. Default `30d`.
    #[arg(long, env = "MOOR_CACHE_MAX_AGE", value_name = "AGE", value_parser = parse_age)]
    cache_max_age: Option<std::time::Duration>,
}

impl ImageFlags {
    /// The host that provides Boxes of `kind`, under `home`.
    fn host(self, kind: &str, home: &std::path::Path) -> Result<Host, String> {
        if kind == "docker" {
            let images = Images {
                agent: self.agent_image,
                work: self.work_image,
                cache_max_age: self.cache_max_age,
            };
            return Ok(Host::docker(images, home));
        }
        Host::named(kind, home)
    }
}

/// Keeps this machine linked until the host stops it, which is a clean end; it fails when the
/// agent cannot run.
fn run_here(
    (allow_exec, boxes, images): (bool, Option<String>, ImageFlags),
    auto_update: bool,
) -> Result<(), String> {
    let machine = Machine {
        allow_exec,
        ..Machine::from_env()
    };
    let host = (!allow_exec)
        .then(|| images.host(boxes.as_deref().unwrap_or(DOCKER), &machine.home))
        .transpose()?;
    let rt = runtime()?;
    let ended = rt.block_on(until_stopped(run_hosting(
        &state_dir(),
        machine,
        host,
        auto_update,
        &|line| say(line),
    )));
    let ended = ended?;
    if let Ended::Stopped(signal) = ended {
        say(&format!("stopped by {signal}"));
    }
    Ok(())
}

/// Installs release `version` in place of this executable and says what changed.
fn update_here(version: &str) -> ExitCode {
    match update(version) {
        Ok(done) => {
            say(&format!("updated from {} to {}", done.from, done.to));
            ExitCode::SUCCESS
        }
        Err(e) => {
            say(&e);
            ExitCode::FAILURE
        }
    }
}

/// A runtime for the agent's async work.
fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start: {e}"))
}

/// Writes `line` to stderr, where the agent logs.
fn say(line: &str) {
    let _ = writeln!(std::io::stderr(), "moor-machine-agent: {line}");
}

/// Shows how a command ended: its last line, or why it failed.
fn done(ended: Result<String, String>) -> ExitCode {
    match ended {
        Ok(line) => {
            say(&line);
            ExitCode::SUCCESS
        }
        Err(e) => {
            say(&e);
            ExitCode::FAILURE
        }
    }
}

/// This host's name, the machine's name when none is given.
fn host_name() -> String {
    rustix::system::uname()
        .nodename()
        .to_string_lossy()
        .into_owned()
}
