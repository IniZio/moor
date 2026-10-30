//! Turns a checked-out person profile into the config an agent reads on the Box.

use moor_core::name::{BoxPath, NameError};
use moor_core::profile::Manifest;
use moor_core::seam::agent_adapter::BoxSetup;

/// What a writer knows about the profile and the Box it writes into.
pub(crate) struct ProfileCtx<'a> {
    /// The parsed `moor-profile.yaml`; empty when the profile has none.
    pub(crate) manifest: &'a Manifest,
    /// Where the profile repo is checked out in the Box.
    pub(crate) profile_dir: &'a BoxPath,
    /// The repo checkout the agent works in.
    pub(crate) workdir: &'a BoxPath,
    /// The Box agent's home.
    pub(crate) home: &'a BoxPath,
}

/// Writes one agent's config from a profile. The step looks at the checkout when it runs, since
/// the files the profile ships are only known on the Box, and it never fails the launch: what
/// went wrong is listed in [`FAILURES_FILE`].
pub(crate) trait ProfileWriter {
    /// Files and a shell step that put the profile's instructions, skills, settings and plugins
    /// where this agent reads them.
    fn write(&self, ctx: &ProfileCtx<'_>) -> Result<BoxSetup, NameError>;
}

/// Under `<workdir>`: one line per profile step that failed, `<what> exit <code>`, rewritten
/// on every run.
pub(crate) const FAILURES_FILE: &str = ".moor/profile-failures";
