//! The built-in presets: tool caches every Box with a repo keeps unless a layer drops them.

use std::num::NonZeroU32;

use super::{CacheName, CachePath, Volume};

/// With no daemon up: fails on a blob whose content is not its name, then removes all but images
/// and build cache, so nothing from the last Box runs again.
const DOCKER_RESTORE: &str = r#"su=; [ "$(id -u)" -eq 0 ] || ! sudo -n true 2>/dev/null || su='sudo -n'
if command -v systemctl >/dev/null 2>&1 && systemctl is-active -q docker.socket docker.service containerd.service 2>/dev/null; then
  mkdir -p "$HOME/.moor" && : >"$HOME/.moor/docker-stopped"
  $su systemctl stop docker.socket docker.service containerd.service || { echo 'Docker could not be stopped'; exit 1; }
fi
blobs=/var/lib/containerd/io.containerd.content.v1.content/blobs/sha256
if $su test -d "$blobs"; then
  $su find "$blobs" -type f -print0 |
    $su xargs -0 -r -P4 -n1 sh -c 'h=$(sha256sum -- "$1") && [ "${h%% *}" = "${1##*/}" ] || { echo "blob ${1##*/} does not match its hash"; exit 255; }' sh || exit 1
fi
$su rm -rf -- "$HOME/docker/containers" "$HOME/docker/volumes" "$HOME/docker/network" /var/lib/containerd/io.containerd.content.v1.content/ingest || { echo 'old containers could not be removed'; exit 1; }"#;

/// Starts again a Docker that [`DOCKER_RESTORE`] stopped, after its volume is clean or empty.
const DOCKER_AFTER: &str = r#"[ -e "$HOME/.moor/docker-stopped" ] || exit 0
rm -f "$HOME/.moor/docker-stopped"
su=; [ "$(id -u)" -eq 0 ] || ! sudo -n true 2>/dev/null || su='sudo -n'
$su systemctl reset-failed docker.socket docker.service containerd.service 2>/dev/null
$su systemctl start docker.socket docker.service"#;

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

/// Size hints of the presets that grow large. Every preset without restore steps shares one disk, whose size
/// is the sum of the hints up to the provider's 40 GB cap, so `cache` and `android-sdk` together ask for all
/// of it. `docker` has a disk of its own and asks for the whole cap too: a Compose build that rebuilds a few
/// images fills the provider's 20 GB default. The disks are sparse, so a repo that never fills them pays nothing.
const SIZE_GB: [(&str, u32); 3] = [("cache", 20), ("android-sdk", 20), ("docker", 40)];

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
            let docker = *name == "docker";
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
                restore: if docker {
                    vec![DOCKER_RESTORE.to_owned()]
                } else {
                    Vec::new()
                },
                after: docker.then_some(DOCKER_AFTER),
            })
        })
        .collect()
}
