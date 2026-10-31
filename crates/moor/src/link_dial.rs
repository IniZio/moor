//! `moor link-dial <machine-id> <slug>`: an ssh `ProxyCommand` that reaches a pool lease's VM
//! through the machine runner's links broker, `<state>/runner/links.sock`.

use std::io::{Read, Write};

use crate::env::Env;
use crate::proxy::links::RunnerLinks;

/// [`link_dial`] on the process's own stdio, as ssh runs a `ProxyCommand`.
#[must_use]
pub fn on_stdio(env: &Env, machine_id: i64, slug: &str) -> u8 {
    link_dial(
        env,
        machine_id,
        slug,
        std::io::stdin(),
        std::io::stdout(),
        std::io::stderr(),
    )
}

/// Bridges `input`/`output` to the sshd of lease `slug` on machine `machine_id` until either
/// side closes; the exit code, 0 once the bridge ends, else 1.
pub fn link_dial(
    env: &Env,
    machine_id: i64,
    slug: &str,
    input: impl Read + Send + 'static,
    output: impl Write,
    mut stderr: impl Write,
) -> u8 {
    let Some(dir) = env.state_dir() else {
        let _ = writeln!(
            stderr,
            "moor: no state directory: set state_dir or MOOR_STATE_DIR"
        );
        return 1;
    };
    let result = match RunnerLinks::under(&dir).dial(machine_id, slug) {
        Ok(conn) => {
            crate::controller::attach::bridge(conn, input, output).map_err(|e| e.to_string())
        }
        Err(e) => Err(e),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(stderr, "moor: link-dial {machine_id} {slug}: {e}");
            1
        }
    }
}
