//! The closed set of named macOS capabilities a Box may be granted.
//!
//! A macOS Box runs the agent under Seatbelt. Every path, preference or Mach
//! grant comes from this table; nothing else may add one.

use std::fmt::Write as _;

/// A named bundle of `Seatbelt` grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MacCapability {
    /// Baseline for any build: the developer toolchain and the working areas.
    Toolchain,
    /// `xcodebuild` state: `DerivedData`, module caches and `Xcode` preferences.
    XcodeBuild,
    /// `ibtool` and `actool`: asset catalogs and interface compilation.
    InterfaceBuilder,
    /// Booting and driving an iOS simulator.
    IosSimulator,
    /// `SwiftPM` build-tool plugins: lets `sandbox-exec` run nested under the sandbox.
    SwiftpmPlugins,
}

impl MacCapability {
    /// Every capability, in grant order.
    pub const ALL: [Self; 5] = [
        Self::Toolchain,
        Self::XcodeBuild,
        Self::InterfaceBuilder,
        Self::IosSimulator,
        Self::SwiftpmPlugins,
    ];

    /// The stable kebab-case name used in config and in rendered lists.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Toolchain => "toolchain",
            Self::XcodeBuild => "xcode-build",
            Self::InterfaceBuilder => "interface-builder",
            Self::IosSimulator => "ios-simulator",
            Self::SwiftpmPlugins => "swiftpm-plugins",
        }
    }

    /// The grants this capability adds, each with the reason it exists.
    #[must_use]
    pub fn grants(self) -> &'static [Reasoned] {
        match self {
            Self::Toolchain => TOOLCHAIN,
            Self::XcodeBuild => XCODE_BUILD,
            Self::InterfaceBuilder => INTERFACE_BUILDER,
            Self::IosSimulator => IOS_SIMULATOR,
            Self::SwiftpmPlugins => SWIFTPM_PLUGINS,
        }
    }
}

/// A base directory a grant is anchored to, resolved by the caller per Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacRoot {
    /// The Box working directory.
    Work,
    /// The Box home directory.
    Home,
    /// The Box temporary directory.
    Tmp,
    /// The Box cache directory.
    Cache,
    /// The simulator device set directory of the Box.
    Sim,
    /// The launch directory the agent starts from.
    Launch,
    /// The active Xcode developer directory.
    DeveloperDir,
    /// The home directory of the user who logged in to the host.
    LoginHome,
    /// The selected host toolchain root at this index, as resolved on the Box.
    HostToolchain(u16),
    /// No anchor: `rel` is an absolute path.
    System,
}

/// A path under a [`MacRoot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacPath {
    /// The anchor directory.
    pub root: MacRoot,
    /// Path below the root; empty means the root itself. For `System`, an absolute path.
    pub rel: &'static str,
}

/// One Seatbelt permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// Read access to a path subtree.
    Read(MacPath),
    /// Write access to a path subtree.
    Write(MacPath),
    /// Access to a preference domain.
    PrefDomain(&'static str),
    /// Lookup of a Mach service by name.
    MachLookup(&'static str),
    /// Running `/usr/bin/sandbox-exec` nested under the sandbox, unsandboxed.
    NestedSandboxExec,
}

/// A grant together with the capability that owns it and why it exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reasoned {
    /// The owning capability.
    pub capability: MacCapability,
    /// The permission.
    pub grant: Grant,
    /// One line saying why the grant exists.
    pub reason: &'static str,
}

/// Which optional capabilities a Box asks for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selection {
    /// Build with Xcode.
    pub xcode: bool,
    /// Use an iOS simulator; implies `xcode`.
    pub ios_simulator: bool,
    /// Run `SwiftPM` build-tool plugins; implies `xcode`.
    pub swiftpm_plugins: bool,
}

/// Capabilities for a selection: sorted, without duplicates, `Toolchain` always first.
#[must_use]
pub fn selected(sel: Selection) -> Vec<MacCapability> {
    let xcode = sel.xcode || sel.ios_simulator || sel.swiftpm_plugins;
    let mut caps = vec![MacCapability::Toolchain];
    if xcode {
        caps.push(MacCapability::XcodeBuild);
        caps.push(MacCapability::InterfaceBuilder);
    }
    if sel.ios_simulator {
        caps.push(MacCapability::IosSimulator);
    }
    if sel.swiftpm_plugins {
        caps.push(MacCapability::SwiftpmPlugins);
    }
    caps.sort();
    caps.dedup();
    caps
}

/// The grants of each capability, in the order given.
#[must_use]
pub fn expand(caps: &[MacCapability]) -> Vec<Reasoned> {
    caps.iter()
        .flat_map(|c| c.grants().iter().copied())
        .collect()
}

/// The grants of `caps`, then one read of each of `host_roots` selected host toolchain roots under the `toolchain` capability.
#[must_use]
pub fn expand_with(caps: &[MacCapability], host_roots: usize) -> Vec<Reasoned> {
    let mut grants = expand(caps);
    if caps.contains(&MacCapability::Toolchain) {
        let roots = u16::try_from(host_roots).unwrap_or(u16::MAX);
        grants.extend((0..roots).map(|i| {
            r(
                TC,
                Grant::Read(p(MacRoot::HostToolchain(i), "")),
                "owner-configured host toolchain",
            )
        }));
    }
    grants
}

/// One line per grant for snapshots: `<capability> <kind> <target> # <reason>`.
#[must_use]
pub fn render_list(grants: &[Reasoned]) -> String {
    let mut out = String::new();
    for g in grants {
        let (kind, target) = match g.grant {
            Grant::Read(p) => ("read", path_target(p)),
            Grant::Write(p) => ("write", path_target(p)),
            Grant::PrefDomain(d) => ("pref", format!("pref:{d}")),
            Grant::MachLookup(n) => ("mach", format!("mach:{n}")),
            Grant::NestedSandboxExec => ("exec", "nested-sandbox-exec".to_owned()),
        };
        let name = g.capability.name();
        let _ = writeln!(out, "{name} {kind} {target} # {}", g.reason);
    }
    out
}

/// Renders a path as `<root>:<rel>`.
fn path_target(p: MacPath) -> String {
    let root = match p.root {
        MacRoot::HostToolchain(i) => format!("host-toolchain[{i}]"),
        MacRoot::Work => "work".to_owned(),
        MacRoot::Home => "home".to_owned(),
        MacRoot::Tmp => "tmp".to_owned(),
        MacRoot::Cache => "cache".to_owned(),
        MacRoot::Sim => "sim".to_owned(),
        MacRoot::Launch => "launch".to_owned(),
        MacRoot::DeveloperDir => "developer-dir".to_owned(),
        MacRoot::LoginHome => "login-home".to_owned(),
        MacRoot::System => "system".to_owned(),
    };
    format!("{root}:{}", p.rel)
}

/// Builds a [`MacPath`].
const fn p(root: MacRoot, rel: &'static str) -> MacPath {
    MacPath { root, rel }
}

/// Builds a [`Reasoned`] entry.
const fn r(capability: MacCapability, grant: Grant, reason: &'static str) -> Reasoned {
    Reasoned {
        capability,
        grant,
        reason,
    }
}

/// Short alias for the toolchain capability in the tables.
const TC: MacCapability = MacCapability::Toolchain;
/// Short alias for the Xcode build capability in the tables.
const XB: MacCapability = MacCapability::XcodeBuild;
/// Short alias for the interface builder capability in the tables.
const IB: MacCapability = MacCapability::InterfaceBuilder;
/// Short alias for the iOS simulator capability in the tables.
const IS: MacCapability = MacCapability::IosSimulator;
/// Short alias for the `SwiftPM` plugins capability in the tables.
const SP: MacCapability = MacCapability::SwiftpmPlugins;

/// Grants of the toolchain capability.
static TOOLCHAIN: &[Reasoned] = &[
    r(
        TC,
        Grant::Read(p(MacRoot::DeveloperDir, "")),
        "compilers and SDKs live in the active developer directory",
    ),
    r(
        TC,
        Grant::Read(p(MacRoot::System, "/Library/Developer")),
        "system-wide developer tools and command line tools",
    ),
    r(
        TC,
        Grant::Read(p(MacRoot::System, "/Library/Apple")),
        "Apple-installed support files the toolchain loads",
    ),
    r(
        TC,
        Grant::Read(p(MacRoot::System, "/var/select")),
        "xcode-select links resolve the active developer directory",
    ),
    r(
        TC,
        Grant::Write(p(MacRoot::Work, "")),
        "the agent edits and builds in the working directory",
    ),
    r(
        TC,
        Grant::Write(p(MacRoot::Home, "")),
        "tools keep per-user state in the Box home",
    ),
    r(
        TC,
        Grant::Write(p(MacRoot::Tmp, "")),
        "tools write scratch files to the temporary directory",
    ),
    r(
        TC,
        Grant::Write(p(MacRoot::Cache, "")),
        "tools write caches to the cache directory",
    ),
    r(
        TC,
        Grant::Read(p(MacRoot::Launch, "")),
        "moor-written launch files are readable but not editable",
    ),
];

/// Grants of the Xcode build capability.
static XCODE_BUILD: &[Reasoned] = &[
    r(
        XB,
        Grant::Write(p(MacRoot::Home, "Library/Developer/Xcode")),
        "DerivedData and module cache",
    ),
    r(
        XB,
        Grant::Write(p(MacRoot::Home, "Library/Caches")),
        "Xcode and clang write caches under the Box home",
    ),
    r(
        XB,
        Grant::PrefDomain("com.apple.dt.Xcode"),
        "Xcode reads and writes its preferences",
    ),
    r(
        XB,
        Grant::PrefDomain("com.apple.dt.xcodebuild"),
        "xcodebuild reads and writes its preferences",
    ),
];

/// Grants of the interface builder capability.
static INTERFACE_BUILDER: &[Reasoned] = &[
    r(
        IB,
        Grant::PrefDomain("com.apple.ibtool"),
        "ibtool reads and writes its preferences",
    ),
    r(
        IB,
        Grant::Read(p(MacRoot::LoginHome, "Library/Developer/CoreSimulator")),
        "actool and ibtool start AssetCatalogSimulatorAgent through CoreSimulator",
    ),
    r(
        IB,
        Grant::Read(p(MacRoot::LoginHome, "Library/Developer/DVTDownloads")),
        "Metal toolchain download directory",
    ),
];

/// Grants of the iOS simulator capability.
static IOS_SIMULATOR: &[Reasoned] = &[
    r(
        IS,
        Grant::Write(p(MacRoot::Sim, "")),
        "the simulator device set of the Box holds device state",
    ),
    r(
        IS,
        Grant::Read(p(MacRoot::System, "/Library/Developer/CoreSimulator")),
        "simulator runtimes are installed system-wide",
    ),
    r(
        IS,
        Grant::Read(p(MacRoot::LoginHome, "Library/Developer/CoreSimulator")),
        "CoreSimulator reads the logged-in user's device data",
    ),
];

/// Grants of the `SwiftPM` plugins capability.
static SWIFTPM_PLUGINS: &[Reasoned] = &[r(
    SP,
    Grant::NestedSandboxExec,
    "Xcode wraps plugin commands in sandbox-exec; any command run through sandbox-exec can leave the sandbox",
)];

#[cfg(test)]
mod tests;
