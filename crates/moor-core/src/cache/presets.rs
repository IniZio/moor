//! The built-in presets: tool caches every Box with a repo keeps unless a layer drops them.

use super::{CacheName, CachePath, Volume};
use crate::exec_env::tool_caches;

/// Clears containers, volumes and networks so only images and build cache are kept; without
/// Docker there is nothing to seal.
const DOCKER_SEAL: &str = "if command -v docker >/dev/null 2>&1; then docker ps -aq | xargs -r docker rm -f && docker volume prune -af && docker network prune -f; fi";

/// Presets whose paths are the tools' own defaults, in `~/` form.
const FIXED: [(&str, &[&str]); 8] = [
    ("pnpm", &["~/.local/share/pnpm/store"]),
    ("bun", &["~/.bun/install/cache"]),
    ("docker", &["~/docker", "/var/lib/containerd"]),
    ("podman", &["~/.local/share/containers"]),
    ("gradle", &["~/.gradle/caches", "~/.gradle/wrapper"]),
    ("dart", &["~/.pub-cache"]),
    ("rust", &["~/.cargo/registry", "~/.cargo/git", "~/.rustup"]),
    ("go", &["~/go/pkg/mod", "~/.cache/go-build"]),
];

/// The names of every preset, in table order.
#[must_use]
pub fn names() -> Vec<&'static str> {
    ["npm", "yarn"]
        .into_iter()
        .chain(FIXED.iter().map(|(n, _)| *n))
        .collect()
}

/// Every preset as a volume. npm and Yarn follow the caches every exec points them at.
pub(super) fn all() -> Vec<Volume> {
    let (npm, yarn) = tool_caches();
    let home = |p: &str| p.replacen("$HOME/", "~/", 1);
    let (npm, yarn) = (home(npm), home(yarn));
    let derived = [("npm", vec![npm.as_str()]), ("yarn", vec![yarn.as_str()])];
    let fixed = FIXED.iter().map(|(n, paths)| (*n, paths.to_vec()));
    derived
        .into_iter()
        .chain(fixed)
        .filter_map(|(name, paths)| {
            let seal = if name == "docker" {
                vec![DOCKER_SEAL.to_owned()]
            } else {
                Vec::new()
            };
            Some(Volume {
                name: CacheName::parse(name).ok()?,
                paths: paths
                    .into_iter()
                    .map(CachePath::parse)
                    .collect::<Result<_, _>>()
                    .ok()?,
                size_gb: None,
                seal,
            })
        })
        .collect()
}
