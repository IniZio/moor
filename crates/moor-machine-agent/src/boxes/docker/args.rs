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

/// The volumes both places mount, `/tmp` and the only parts of the agent's home they share. The work
/// place's `npx skills add -g` writes them and Claude, in the agent place, loads them, while the
/// rest of each home stays apart so a wrecked work place never takes Claude's session with it.
fn shared_skills(n: &Names) -> Vec<String> {
    let mut a = owned(["-v", &format!("{}:/home/agent/.agents", n.agents)]);
    a.extend(owned([
        "-v",
        &format!("{}:/home/agent/.claude/skills", n.skills),
    ]));
    a.extend(owned(["-v", &format!("{}:/tmp", n.tmp)]));
    a
}

/// The argv, after `docker`, that starts the agent place: not privileged, on the Box network,
/// and never restarted by Docker, because the machine agent is the only actor that restarts a Box.
pub(super) fn agent_run(s: &Settings, slug: &Slug, n: &Names, cache: &[String]) -> Vec<String> {
    let mut a = owned(["run", "-d", "--name", &n.agent, "--hostname", slug.as_str()]);
    a.extend(owned(["--network", &s.network, "--restart", "no"]));
    a.extend(owned(["--memory", AGENT_MEMORY]));
    a.extend(owned([
        "--label",
        &format!("{SLUG_LABEL}={}", slug.as_str()),
    ]));
    a.extend(owned(["--label", &format!("{PLACE_LABEL}=agent")]));
    a.extend(owned(["-v", &format!("{}:{WORKDIR}", n.tree)]));
    a.extend(owned(["-v", &format!("{}:/run/moor", n.run)]));
    a.extend(owned(["-v", &format!("{}:/home/agent", n.agent_home)]));
    a.extend(shared_skills(n));
    a.extend(cache.iter().cloned());
    a.push(s.agent_image.clone());
    a
}

/// The argv, after `docker`, that starts the work place: privileged, in the agent place's network
/// namespace, with Docker of its own inside. It opens the host's `/dev/kvm`, which `up` checks
/// is open to everyone, since rootless Docker drops the host's `kvm` group.
pub(super) fn work_run(
    s: &Settings,
    slug: &Slug,
    n: &Names,
    mem_mib: u64,
    cache: &[String],
) -> Vec<String> {
    let mem = format!("{mem_mib}m");
    let mut a = owned(["run", "-d", "--name", &n.work]);
    a.extend(owned(["--network", &format!("container:{}", n.agent)]));
    a.extend(owned(["--privileged", "--restart", "no"]));
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
    a.extend(cache.iter().cloned());
    a.push(s.work_image.clone());
    a
}
