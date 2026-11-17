//! The `docker run` argv of each place. Pure, so a test can read them without a daemon.

use super::names::Names;
use super::Settings;
use crate::boxes::Slug;

/// Where the work tree is mounted in both places.
pub(super) const WORKDIR: &str = "/home/agent/moor";
/// The memory limit of the agent place.
const AGENT_MEMORY: &str = "2g";
/// The label that says which Box a container belongs to.
pub(super) const SLUG_LABEL: &str = "moor.slug";
/// The label that says which place of its Box a container is.
pub(super) const PLACE_LABEL: &str = "moor.place";

/// The work place's memory in MiB for a machine class, as the classes are sized everywhere; a
/// medium one when unnamed or unknown.
pub(super) fn work_memory_mib(class: Option<&str>) -> u64 {
    match class {
        Some("small") => 4096,
        Some("large") => 16_384,
        Some("xlarge") => 32_768,
        _ => 8192,
    }
}

/// Owned copies of `words`.
fn owned<const N: usize>(words: [&str; N]) -> Vec<String> {
    words.iter().map(|w| (*w).to_owned()).collect()
}

/// The two volumes both places mount, the only part of the agent's home they share. The work
/// place's `npx skills add -g` writes them and Claude, in the agent place, loads them, while the
/// rest of each home stays apart so a wrecked work place never takes Claude's session with it.
fn shared_skills(n: &Names) -> Vec<String> {
    let mut a = owned(["-v", &format!("{}:/home/agent/.agents", n.agents)]);
    a.extend(owned([
        "-v",
        &format!("{}:/home/agent/.claude/skills", n.skills),
    ]));
    a
}

/// The argv, after `docker`, that starts the agent place: not privileged, on the Box network,
/// and never restarted by Docker, because the machine agent is the only actor that restarts a Box.
pub(super) fn agent_run(s: &Settings, slug: &Slug, n: &Names) -> Vec<String> {
    let mut a = owned(["run", "-d", "--name", &n.agent, "--hostname", slug.as_str()]);
    a.extend(owned(["--network", &s.network, "--restart", "no"]));
    a.extend(owned(["--memory", AGENT_MEMORY]));
    a.extend(owned(["--add-host", "moor-host:host-gateway"]));
    a.extend(owned([
        "--label",
        &format!("{SLUG_LABEL}={}", slug.as_str()),
    ]));
    a.extend(owned(["--label", &format!("{PLACE_LABEL}=agent")]));
    a.extend(owned(["-v", &format!("{}:{WORKDIR}", n.tree)]));
    a.extend(owned(["-v", &format!("{}:/run/moor", n.run)]));
    a.extend(owned(["-v", &format!("{}:/home/agent", n.agent_home)]));
    a.extend(shared_skills(n));
    a.push(s.agent_image.clone());
    a
}

/// The host's KVM device.
const KVM: &str = "/dev/kvm";

/// The group that owns the host's `/dev/kvm`; none when the host has no such device. The gid is
/// the host's, so the work image cannot know it.
pub(super) fn kvm_gid() -> Option<u32> {
    gid_of(std::path::Path::new(KVM))
}

/// The group that owns `device`, when it exists.
pub(super) fn gid_of(device: &std::path::Path) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(device).ok().map(|m| m.gid())
}

/// The argv, after `docker`, that starts the work place: privileged, in the agent place's network
/// namespace, with Docker of its own inside. With `kvm_gid` the work init puts the Box user in
/// that group so it can open the host's `/dev/kvm`. Docker's `--group-add` would not do: the ssh
/// login that runs commands rebuilds the user's groups from `/etc/group`.
pub(super) fn work_run(
    s: &Settings,
    slug: &Slug,
    n: &Names,
    mem_mib: u64,
    kvm_gid: Option<u32>,
) -> Vec<String> {
    let mem = format!("{mem_mib}m");
    let mut a = owned(["run", "-d", "--name", &n.work]);
    a.extend(owned(["--network", &format!("container:{}", n.agent)]));
    a.extend(owned(["--privileged", "--restart", "no"]));
    if let Some(gid) = kvm_gid {
        a.extend(owned(["-e", &format!("MOOR_KVM_GID={gid}")]));
    }
    a.extend(owned(["--memory", &mem, "--memory-swap", &mem]));
    a.extend(owned([
        "--label",
        &format!("{SLUG_LABEL}={}", slug.as_str()),
    ]));
    a.extend(owned(["--label", &format!("{PLACE_LABEL}=work")]));
    a.extend(owned(["-v", &format!("{}:{WORKDIR}", n.tree)]));
    a.extend(owned(["-v", &format!("{}:/run/moor", n.run)]));
    a.extend(owned(["-v", &format!("{}:/home/agent", n.work_home)]));
    a.extend(shared_skills(n));
    a.extend(owned(["-v", &format!("{}:/var/lib/docker", n.docker)]));
    a.push(s.work_image.clone());
    a
}
