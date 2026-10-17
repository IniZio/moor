//! The sandbox profile of one run, built the same way for the agent and for the other Box execs.

use moor_core::config::{BoxCapability, RepoBox};
use moor_core::mac_capability::{selected, MacCapability, Selection};
use moor_core::name::BoxPath;
use moor_core::seam::engine::MacGrants;

use super::super::sandbox::mac::MacInput;
use super::super::sandbox::{self, SandboxInput};
use super::mac::MacBox;
use super::{fail, Failure};

/// What a profile is built from.
pub(in super::super) struct ProfileArgs<'a> {
    /// The Box's operating system as `uname -s` reports it.
    pub(in super::super) os: &'a str,
    /// The directory the command runs in.
    pub(in super::super) workdir: &'a BoxPath,
    /// The home the command sees: the Task's own on a macOS Box.
    pub(in super::super) home: &'a BoxPath,
    /// The credential directory.
    pub(in super::super) cred: &'a BoxPath,
    /// The temp directory.
    pub(in super::super) tmp: &'a BoxPath,
    /// Names to pass through from the environment.
    pub(in super::super) env_names: Vec<String>,
    /// Variables the sandbox sets for the command.
    pub(in super::super) set_vars: Vec<(String, String)>,
    /// The Box user is root.
    pub(in super::super) root: bool,
    /// Names that never reach the sandbox.
    pub(in super::super) strip_env: &'a [String],
    /// The Task's Box, on a macOS layout.
    pub(in super::super) mac: Option<&'a MacBox>,
    /// What the repo grants the sandbox.
    pub(in super::super) grants: &'a MacGrants,
}

/// The profile text for `args`.
///
/// # Errors
/// Capabilities that cannot be granted here, or grants that cannot be rendered.
pub(in super::super) fn build(args: ProfileArgs) -> Result<String, Failure> {
    let caps = mac_grants(args.os, args.mac.is_some(), args.grants)?;
    sandbox::profile(&SandboxInput {
        os: args.os,
        workdir: args.workdir,
        home: args.home,
        cred: args.cred,
        tmp: args.tmp,
        env_names: args.env_names,
        set_vars: args.set_vars,
        root: args.root,
        strip_env: args.strip_env,
        mac: args.mac.zip(caps.as_deref()).map(|(m, caps)| MacInput {
            layout: &m.layout,
            login_home: &m.login_home,
            developer_dir: m.developer_dir.as_str(),
            capabilities: caps,
            sandbox: &args.grants.sandbox,
            toolchains: &m.toolchains,
        }),
    })
    .map_err(|e| fail("sandbox", &e))
}

/// The capabilities to grant, when a Darwin Box runs the Task inside a Mac layout.
///
/// # Errors
/// A run that asks for capabilities but cannot be granted them (not a Darwin Box, or no Mac
/// layout for the workdir) fails with the same text as the provision-time platform check.
fn mac_grants(
    os: &str,
    in_layout: bool,
    grants: &MacGrants,
) -> Result<Option<Vec<MacCapability>>, Failure> {
    if os != "Darwin" || !in_layout {
        let asked = RepoBox {
            capabilities: grants.capabilities.clone(),
            toolchains: grants.toolchains.iter().map(|t| t.name.clone()).collect(),
            ..RepoBox::default()
        };
        return match asked.check_platform(false) {
            Ok(()) => Ok(None),
            Err(e) => Err(fail("sandbox", &e.in_file(".moor.yaml"))),
        };
    }
    let has = |c| grants.capabilities.contains(&c);
    let sel = Selection {
        xcode: has(BoxCapability::Xcode),
        ios_simulator: has(BoxCapability::IosSimulator),
        swiftpm_plugins: has(BoxCapability::SwiftpmPlugins),
    };
    Ok(Some(selected(sel)))
}
