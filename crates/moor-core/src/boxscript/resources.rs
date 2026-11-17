//! Script that reads what CPUs and memory a Box has, and the parser of what it prints.

use crate::shape::Shape;

/// POSIX sh that prints `moor.cpus=N` and `moor.mem_mib=N`; it never exits.
///
/// `nproc` counts the CPUs the Box may run on, so a cpuset smaller than the machine shows. A
/// container's `/proc/meminfo` names its host's memory, so a cgroup limit below that wins.
pub const PROBE: &str = "echo moor.cpus=$(nproc 2>/dev/null); \
mem=$(awk '/^MemTotal:/ {print int($2/1024)}' /proc/meminfo 2>/dev/null); \
for f in /sys/fs/cgroup/memory.max /sys/fs/cgroup/memory/memory.limit_in_bytes; do \
lim=$(cat $f 2>/dev/null); \
case $lim in ''|*[!0-9]*) continue;; esac; \
lim=$((lim/1048576)); \
if [ \"$lim\" -gt 0 ] && [ \"$lim\" -lt \"${mem:-0}\" ]; then mem=$lim; fi; \
done; \
echo moor.mem_mib=$mem; ";

/// `script` followed by [`PROBE`], exiting with `script`'s own status.
///
/// A script that exits early never reaches the probe, which only matters for a failure.
#[must_use]
pub fn after(script: &str) -> String {
    format!("{script}\nrc=$?\n{PROBE}\nexit $rc\n")
}

/// The shape [`PROBE`] printed in `output`; `None` unless both lines carry a positive number.
#[must_use]
pub fn parse(output: &str) -> Option<Shape> {
    let value = |key: &str| {
        output.lines().find_map(|line| {
            let (k, v) = line.trim_end().split_once('=')?;
            (k == key).then(|| v.parse::<u64>().ok())?
        })
    };
    let cpus = u32::try_from(value("moor.cpus").filter(|c| *c > 0)?).ok()?;
    let mem_mib = value("moor.mem_mib").filter(|m| *m > 0)?;
    Some(Shape { cpus, mem_mib })
}

#[cfg(test)]
mod tests;
