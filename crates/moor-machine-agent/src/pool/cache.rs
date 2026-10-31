//! A warmup's cache volumes, checked against crabbox's schema before they reach it: the
//! controller gets no say over this host beyond the volumes crabbox itself would accept.

use moor_client::link::lease::{Cache, CacheVolume};
use moor_core::cache::{CacheName, CachePath};

/// The most volumes one warmup mounts.
const MOST_VOLUMES: usize = 32;
/// The most paths one volume spans.
const MOST_PATHS: usize = 16;
/// The largest size hint, in GB.
const MOST_GB: u32 = 1024;
/// The longest `CRABBOX_CACHE_VOLUMES` value.
const MOST_BYTES: usize = 64 * 1024;

/// The env crabbox's warmup reads `cache` from; why it is refused.
pub(super) fn env(cache: &Cache) -> Result<Vec<(&'static str, String)>, String> {
    let count = cache.volumes.len();
    if count == 0 || count > MOST_VOLUMES {
        return Err(format!(
            "a cache names 1 to {MOST_VOLUMES} volumes, not {count}"
        ));
    }
    for v in &cache.volumes {
        volume(v).map_err(|why| format!("cache volume {:?}: {why}", v.key))?;
    }
    let volumes = serde_json::to_string(&cache.volumes).map_err(|e| e.to_string())?;
    if volumes.len() > MOST_BYTES {
        return Err(format!("a cache spec is at most {MOST_BYTES} bytes"));
    }
    let mut env = vec![("CRABBOX_CACHE_VOLUMES", volumes)];
    if let Some(scope) = &cache.scope {
        let hex = scope.len() == 16
            && scope
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if !hex {
            return Err(format!(
                "cache scope {scope:?} must be 16 lowercase hex digits"
            ));
        }
        env.push(("CRABBOX_CACHE_SCOPE", scope.clone()));
    }
    Ok(env)
}

/// Ok when `v` is a volume crabbox's external provider takes and its paths lie in the VM.
fn volume(v: &CacheVolume) -> Result<(), String> {
    let token = v.group.len() <= 96
        && !matches!(v.group.as_str(), "" | "." | "..")
        && v.group
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if !token {
        return Err(format!(
            "group {:?} must match [A-Za-z0-9._-]{{1,96}}",
            v.group
        ));
    }
    CacheName::parse(&v.name).map_err(|e| e.to_string())?;
    if v.key != format!("{}-{}", v.group, v.name) {
        return Err("key must be <group>-<name>".to_owned());
    }
    if v.paths.is_empty() || v.paths.len() > MOST_PATHS {
        return Err(format!("a volume spans 1 to {MOST_PATHS} paths"));
    }
    for path in &v.paths {
        CachePath::parse(path).map_err(|e| e.to_string())?;
    }
    if v.size_gb > MOST_GB {
        return Err(format!("sizeGB is at most {MOST_GB}"));
    }
    Ok(())
}
