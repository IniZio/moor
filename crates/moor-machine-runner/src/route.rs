//! Where a command for a Box runs once ssh reaches its machine: on the machine itself, or in
//! the Box's VM on it, one ssh hop further with the words crabbox printed on the host. The VM's
//! key never leaves the host, since the second hop starts there.

use crate::exec::quote;

/// The last hop of a Box's ssh calls.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum Route {
    /// The command runs on the machine the ssh target names.
    #[default]
    Host,
    /// The command runs in the VM these ssh words reach from the machine.
    Vm(Vec<String>),
}

impl Route {
    /// The remote command that runs `command` at the end of the route, for the host's shell.
    pub(crate) fn remote(&self, command: &str) -> String {
        match self {
            Self::Host => command.to_owned(),
            Self::Vm(words) => format!("{} {}", hop(words), quote(command)),
        }
    }

    /// rsync's `--rsync-path` for `rsync`, a remote command that ends in rsync and takes
    /// rsync's server arguments after it.
    pub(crate) fn rsync_path(&self, rsync: &str) -> String {
        match self {
            Self::Host => rsync.to_owned(),
            // ssh joins its command words with spaces, so the server arguments rsync appends
            // on the host land after `rsync` in the VM.
            Self::Vm(words) => format!("{} {}", hop(words), quote(rsync)),
        }
    }
}

/// The host's ssh to the VM, before the remote command.
fn hop(words: &[String]) -> String {
    let words: Vec<String> = words.iter().map(|w| quote(w)).collect();
    format!("ssh -o BatchMode=yes {}", words.join(" "))
}

#[cfg(test)]
mod tests {
    use super::Route;

    #[test]
    fn the_host_route_leaves_commands_alone() {
        assert_eq!(Route::Host.remote("cd x && ls"), "cd x && ls");
        assert_eq!(Route::Host.rsync_path("rsync"), "rsync");
    }

    #[test]
    fn the_vm_route_wraps_commands_in_a_second_hop() {
        let vm = Route::Vm(vec!["-p".into(), "2222".into(), "vm@10.0.0.5".into()]);
        assert_eq!(
            vm.remote("echo 'hi'"),
            r"ssh -o BatchMode=yes '-p' '2222' 'vm@10.0.0.5' 'echo '\''hi'\'''"
        );
    }
}
