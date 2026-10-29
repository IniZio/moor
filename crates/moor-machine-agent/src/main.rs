//! `moor-machine-agent`: enrolls this computer with a moor server and keeps it linked.

use std::io::Write;
use std::process::ExitCode;
use std::sync::mpsc::Sender;

use clap::{Parser, Subcommand};
use moor_client::oidc::{Console, Ui, REDIRECT_PORT};
use moor_machine_agent::{enroll, run, state_dir, Enroll, Machine};

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
    /// Signs in as this machine's owner and enrolls it.
    Enroll {
        /// The moor server, such as `https://moor.example.com`.
        #[arg(long, env = "MOOR_SERVER")]
        server: String,
        /// The machine's name, unique among your machines.
        #[arg(long)]
        name: String,
        /// Labels a Box can ask for, comma separated.
        #[arg(long, value_delimiter = ',')]
        labels: Vec<String>,
        /// How many Boxes may hold this machine at once.
        #[arg(long, default_value_t = 1)]
        slots: u16,
        /// Do not open a browser; paste the address the sign-in ends on.
        #[arg(long)]
        paste: bool,
        /// Make a new key instead of keeping this machine's.
        #[arg(long)]
        new_key: bool,
    },
    /// Keeps this enrolled machine linked to its server until stopped.
    Run,
}

/// The console, with or without the browser.
struct Terminal {
    /// Opens the sign-in in the platform's browser.
    browser: bool,
}

impl Ui for Terminal {
    fn say(&mut self, line: &str) {
        Console.say(line);
    }

    fn open(&mut self, url: &str) {
        if self.browser {
            Console.open(url);
        }
    }

    fn pasted(&mut self, to: Sender<String>) {
        Console.pasted(to);
    }
}

fn main() -> ExitCode {
    let ended = match Args::parse().command {
        Command::Enroll {
            server,
            name,
            labels,
            slots,
            paste,
            new_key,
        } => {
            let req = Enroll {
                server,
                name,
                labels,
                slots,
                new_key,
            };
            return enroll_here(&req, paste);
        }
        Command::Run => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("cannot start: {e}"))
            .and_then(|rt| rt.block_on(run(&state_dir(), Machine::from_env(), &|line| say(line)))),
    };
    if let Err(e) = ended {
        say(&e);
    }
    ExitCode::FAILURE
}

/// Writes `line` to stderr, where the agent logs.
fn say(line: &str) {
    let _ = writeln!(std::io::stderr(), "moor-machine-agent: {line}");
}

/// Enrolls this machine, signing in through the browser unless `paste`.
fn enroll_here(req: &Enroll, paste: bool) -> ExitCode {
    let mut ui = Terminal { browser: !paste };
    match enroll(req, &state_dir(), REDIRECT_PORT, &mut ui) {
        Ok(state) => {
            ui.say(&format!(
                "Enrolled {} as machine {} for {}.",
                state.name, state.machine_id, state.owner
            ));
            ExitCode::SUCCESS
        }
        Err(e) => {
            say(&e);
            ExitCode::FAILURE
        }
    }
}
