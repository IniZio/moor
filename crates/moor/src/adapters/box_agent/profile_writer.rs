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

/// Records a failed step as one line and goes on. Called `fail <what> $?` right after the command.
pub(crate) const FAIL_FN: &str =
    r#"fail() { echo "moor: profile: $1 exit $2" >&2; printf '%s exit %s\n' "$1" "$2" >> "$f"; }"#;

/// Copies each person skill under `$1`, replacing one of the same name. A directory without a
/// `SKILL.md` is not a skill.
pub(crate) const COPY_SKILLS: &str = r#"for s in "$d"/skills/*/; do
  s=${s%/}; [ -f "$s/SKILL.md" ] || continue; n=${s##*/}
  rm -rf "$1/$n"; cp -R "$s" "$1/$n" || fail "skill $n" $?
done"#;

/// Reads the `name` of a marketplace manifest, `$1`, with whichever of python or node exists.
pub(crate) const MARKETPLACE_NAME_FN: &str = r#"mkt_name() {
  if command -v python3 >/dev/null 2>&1; then python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["name"])' "$1"
  elif command -v node >/dev/null 2>&1; then node -e 'console.log(require(process.argv[1]).name)' "$1"
  else return 1; fi 2>/dev/null
}"#;
