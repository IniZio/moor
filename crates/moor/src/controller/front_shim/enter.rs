//! Entering the front's namespaces from the outer shim: dying with the controller, the user,
//! mount and network namespaces with the front's loopback up, then the PID namespace, and
//! passing `CAP_SYS_ADMIN` on to pid 1.

use std::fs;

use nix::sched::{unshare, CloneFlags};
use rustix::mount::{mount_change, MountPropagationFlags};
use rustix::process::{getgid, getppid, getuid, set_parent_process_death_signal, Pid, Signal};
use rustix::thread::{
    capabilities, configure_capability_in_ambient_set, set_capabilities, CapabilitySet,
};

use super::net;

/// Keeps `CAP_SYS_ADMIN` in the namespaces for the next program this process runs, pid 1,
/// which mounts the front's root.
pub(super) fn pass_sys_admin() -> Result<(), String> {
    let mut caps = capabilities(None).map_err(|e| format!("capget: {e}"))?;
    caps.inheritable |= CapabilitySet::SYS_ADMIN;
    set_capabilities(None, caps).map_err(|e| format!("capset: {e}"))?;
    configure_capability_in_ambient_set(CapabilitySet::SYS_ADMIN, true)
        .map_err(|e| format!("cannot pass CAP_SYS_ADMIN to pid 1: {e}"))
}

/// Makes this process die with `parent`, the controller that started it: the kernel kills it
/// when the controller exits, and it exits now if the controller already did.
pub(super) fn die_with(parent: Pid) -> Result<(), String> {
    set_parent_process_death_signal(Some(Signal::KILL))
        .map_err(|e| format!("cannot set the parent death signal: {e}"))?;
    if getppid() != Some(parent) {
        return Err(format!(
            "the controller (pid {}) is gone",
            parent.as_raw_nonzero()
        ));
    }
    Ok(())
}

/// Why a host refuses the namespaces, with each sysctl that allows them.
fn refused(e: nix::errno::Errno) -> String {
    format!(
        "they need unprivileged user namespaces (sysctl kernel.unprivileged_userns_clone=1, \
         user.max_user_namespaces above 0, kernel.apparmor_restrict_unprivileged_userns=0): \
         cannot create user, mount, PID and network namespaces: {e}"
    )
}

/// Enters new user, mount and network namespaces as the same uid and gid, with mounts that
/// no longer reach the host's and a network with only its loopback, which it brings up. The
/// PID namespace comes after, in [`enter_pid_namespace`], because a process that has unshared
/// one can start no thread, and the outer shim starts its network threads in between.
pub(super) fn isolate() -> Result<(), String> {
    let (uid, gid) = (getuid().as_raw(), getgid().as_raw());
    unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWNET)
        .map_err(refused)?;
    for (file, text) in [
        ("setgroups", "deny".to_owned()),
        ("uid_map", format!("{uid} {uid} 1")),
        ("gid_map", format!("{gid} {gid} 1")),
    ] {
        fs::write(format!("/proc/self/{file}"), text)
            .map_err(|e| format!("cannot write /proc/self/{file}: {e}"))?;
    }
    mount_change(
        "/",
        MountPropagationFlags::PRIVATE | MountPropagationFlags::REC,
    )
    .map_err(|e| format!("cannot make mounts private: {e}"))?;
    net::loopback_up()
}

/// Enters a new PID namespace, whose pid 1 is the next child.
pub(super) fn enter_pid_namespace() -> Result<(), String> {
    unshare(CloneFlags::CLONE_NEWPID).map_err(refused)
}
