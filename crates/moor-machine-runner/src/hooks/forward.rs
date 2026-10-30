//! The hop from a VM host's loopback into a Box's VM: the controller serves each Box port on
//! the host's loopback, moved by the slot, and one ssh per Box carries it on to the repo's own
//! port on the VM's loopback.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::caller::{Caller, Reach};
use crate::exec::quote;

/// The first wait before a forward that ended starts again.
const FIRST_BACKOFF: Duration = Duration::from_secs(1);
/// The longest wait before a forward starts again.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// One Box's forward: what it carries and the task that keeps it up.
struct Running {
    /// The ssh target, VM words and `(vm port, host port)` pairs it was started with.
    key: (String, Vec<String>, Vec<(u16, u16)>),
    /// Keeps the ssh up; aborting it kills the ssh.
    task: JoinHandle<()>,
}

/// The running forwards, by Box id.
#[derive(Default)]
pub(crate) struct VmForwards(Mutex<HashMap<i64, Running>>);

impl VmForwards {
    /// Keeps the caller's forward up from its host through `words`; one already up with the
    /// same ports stays as it is. A host the runner does not reach over ssh gets none.
    pub(crate) fn start(&self, caller: &Caller, words: &[String]) {
        let Reach::Ssh { ssh } = &caller.reach else {
            return;
        };
        if caller.ports.is_empty() {
            return;
        }
        let id = caller.box_id.get();
        let key = (ssh.clone(), words.to_vec(), caller.vm_ports());
        let mut running = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if running.get(&id).is_some_and(|r| r.key == key) {
            return;
        }
        let task = tokio::spawn(keep(id, key.clone()));
        if let Some(old) = running.insert(id, Running { key, task }) {
            old.task.abort();
        }
    }

    /// Ends Box `id`'s forward, if it has one.
    pub(crate) fn end(&self, id: i64) {
        let old = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        if let Some(old) = old {
            old.task.abort();
        }
    }
}

/// Runs the forward's ssh, and again whenever it ends, waiting longer each time it ends soon.
async fn keep(id: i64, (ssh, words, ports): (String, Vec<String>, Vec<(u16, u16)>)) {
    let mut backoff = FIRST_BACKOFF;
    loop {
        let started = Instant::now();
        let ended = run(&ssh, &words, &ports).await;
        if started.elapsed() > MAX_BACKOFF {
            backoff = FIRST_BACKOFF;
        }
        eprintln!(
            "moor-machine-runner: the port forward of Box {id} ended ({ended}); again in {} s",
            backoff.as_secs()
        );
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Runs the forward's ssh until it exits; how it ended.
async fn run(ssh: &str, words: &[String], ports: &[(u16, u16)]) -> String {
    let child = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "--", ssh, &remote(words, ports)])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => return format!("could not start ssh: {e}"),
    };
    // `wait` would close stdin, and its EOF is what ends the forward on the host.
    let _stdin = child.stdin.take();
    match child.wait().await {
        Ok(status) => status.to_string(),
        Err(e) => e.to_string(),
    }
}

/// The host's script that serves each host port as its VM port on the VM's loopback until the
/// runner's ssh closes stdin. `ssh -N` ignores stdin, so a watcher ends it; the watcher reads
/// fd 3, since sh gives a background job /dev/null for stdin.
fn remote(words: &[String], ports: &[(u16, u16)]) -> String {
    format!(
        "exec 3<&0\n{} </dev/null 3<&- & p=$!\n\
         (cat <&3 >/dev/null; kill $p) >/dev/null 2>&1 & w=$!\n\
         exec 3<&-\nwait $p; s=$?; kill $w 2>/dev/null; exit $s",
        inner(words, ports)
    )
}

/// The host's ssh into the VM that carries the ports.
fn inner(words: &[String], ports: &[(u16, u16)]) -> String {
    let mut args: Vec<String> = [
        "ssh",
        "-o",
        "BatchMode=yes",
        "-o",
        "ExitOnForwardFailure=yes",
        "-N",
    ]
    .map(str::to_owned)
    .into();
    for (vm, host) in ports {
        args.push("-R".to_owned());
        args.push(format!("{vm}:127.0.0.1:{host}"));
    }
    args.extend(words.iter().map(|w| quote(w)));
    args.join(" ")
}

#[cfg(test)]
mod tests {
    use super::inner;

    #[test]
    fn serves_each_host_port_as_its_vm_port() {
        let words = vec!["-p".to_owned(), "2222".to_owned(), "vm@vm7".to_owned()];
        assert_eq!(
            inner(&words, &[(8081, 8101), (3000, 3020)]),
            "ssh -o BatchMode=yes -o ExitOnForwardFailure=yes -N \
             -R 8081:127.0.0.1:8101 -R 3000:127.0.0.1:3020 '-p' '2222' 'vm@vm7'"
        );
    }
}
