use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

pub(super) fn proxy_pids(state: &Path) -> Vec<u32> {
    let Ok(entries) = fs::read_dir(state.join("proxy")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .filter_map(|v| v["pid"].as_u64())
        .map(|p| p as u32)
        .filter(|p| *p > 1)
        .collect()
}

pub(super) struct Marked {
    pub(super) pid: u32,
    pub(super) pgrp: libc::pid_t,
    pub(super) cmdline: String,
}

pub(super) struct Stat {
    pub(super) zombie: bool,
    pub(super) pgrp: libc::pid_t,
    pub(super) starttime: u64,
}

pub(super) fn proc_stat(pid: u32) -> Option<Stat> {
    let raw = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = raw.rsplit_once(')')?;
    let f: Vec<&str> = rest.split_whitespace().collect();
    Some(Stat {
        zombie: f.first()? == &"Z",
        pgrp: f.get(2)?.parse().ok()?,
        starttime: f.get(19)?.parse().ok()?,
    })
}

pub(super) fn find_marked(marker: &str) -> Vec<Marked> {
    let me = std::process::id();
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == me {
            continue;
        }
        let Ok(env) = fs::read(entry.path().join("environ")) else {
            continue;
        };
        if !env.split(|b| *b == 0).any(|e| e == marker.as_bytes()) {
            continue;
        }
        let Some(st) = proc_stat(pid) else { continue };
        if st.zombie {
            continue;
        }
        let raw = fs::read(entry.path().join("cmdline")).unwrap_or_default();
        let cmdline = String::from_utf8_lossy(&raw).replace('\0', " ");
        out.push(Marked {
            pid,
            pgrp: st.pgrp,
            cmdline: cmdline.trim_end().to_string(),
        });
    }
    out
}

pub(super) fn wait_marked(marker: &str, limit: Duration) -> Vec<Marked> {
    let deadline = Instant::now() + limit;
    let mut left = find_marked(marker);
    while !left.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        left = find_marked(marker);
    }
    left
}

pub(super) fn creds_in(dir: &Path) -> BTreeSet<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("moor-cred-"))
        .collect()
}
