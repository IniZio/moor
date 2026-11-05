//! The built-in presets: tool caches every Box with a repo keeps unless a layer drops them.

use std::num::NonZeroU32;

use super::{CacheName, CachePath, OnFail, Volume};

/// Clears containers, volumes and networks so only images and build cache are kept; without
/// Docker there is nothing to seal. The login user may lack the socket, so `sudo -n` is tried
/// next; a Docker neither can reach fails the seal.
const DOCKER_SEAL: &str = "command -v docker >/dev/null 2>&1 || exit 0; \
    if docker info >/dev/null 2>&1; then d=docker; \
    elif sudo -n docker info >/dev/null 2>&1; then d='sudo -n docker'; else exit 1; fi; \
    $d ps -aq | xargs -r $d rm -f && $d volume prune -af && $d network prune -f";

/// Presets whose paths are the tools' own defaults, in `~/` form.
///
/// `cache` is the XDG cache root, so npm, Yarn, Go's build cache, pip, ccache and every other
/// tool that follows XDG live in it. Each preset below keeps only what a tool downloads or
/// rebuilds. The tools' own config and keys sit beside it (`~/.m2/settings.xml`,
/// `~/.android/adbkey`, `~/.bundle/config`) and stay out.
const FIXED: [(&str, &[&str]); 14] = [
    ("cache", &["~/.cache"]),
    ("pnpm", &["~/.local/share/pnpm/store"]),
    ("bun", &["~/.bun/install/cache"]),
    ("docker", &["~/docker", "/var/lib/containerd"]),
    ("podman", &["~/.local/share/containers"]),
    ("gradle", &["~/.gradle/caches", "~/.gradle/wrapper"]),
    ("dart", &["~/.pub-cache"]),
    ("rust", &["~/.cargo/registry", "~/.cargo/git", "~/.rustup"]),
    ("go", &["~/go/pkg/mod"]),
    ("maven", &["~/.m2/repository"]),
    ("cocoapods", &["~/.cocoapods/repos"]),
    ("bundler", &["~/.bundle/cache", "~/.local/share/gem"]),
    ("android", &["~/.android/cache", "~/.android/build-cache"]),
    ("android-sdk", &["/opt/android-sdk"]),
];

/// Size hints of the presets that grow large. Every unsealed preset shares one disk, whose size
/// is the sum of the hints up to the provider's 40 GB cap, so these two together ask for all of it.
/// The disk is sparse, so a repo that never fills it pays nothing.
const SIZE_GB: [(&str, u32); 2] = [("cache", 20), ("android-sdk", 20)];

/// The names of every preset, in table order.
#[must_use]
pub fn names() -> Vec<&'static str> {
    FIXED.iter().map(|(n, _)| *n).collect()
}

/// Every preset as a volume.
pub(super) fn all() -> Vec<Volume> {
    FIXED
        .iter()
        .filter_map(|(name, paths)| {
            let seal = if *name == "docker" {
                vec![DOCKER_SEAL.to_owned()]
            } else {
                Vec::new()
            };
            Some(Volume {
                name: CacheName::parse(name).ok()?,
                paths: paths
                    .iter()
                    .map(|p| CachePath::parse(p))
                    .collect::<Result<_, _>>()
                    .ok()?,
                size_gb: SIZE_GB
                    .iter()
                    .find(|(n, _)| n == name)
                    .and_then(|(_, gb)| NonZeroU32::new(*gb)),
                seal,
                on_fail: OnFail::Clear,
            })
        })
        .collect()
}
