//! The read-only launch files and directories of one macOS Box Task.

use super::MacLayout;
use crate::boxscript::quote;
use crate::name::{BoxPath, NameError};

impl MacLayout {
    /// `<launch>/moor.xcconfig`.
    ///
    /// # Errors
    /// Returns a [`NameError`] when the path cannot be joined.
    pub fn xcconfig_file(&self) -> Result<BoxPath, NameError> {
        BoxPath::under(self.launch.as_str(), "moor.xcconfig")
    }

    /// `<launch>/developer-dir`.
    ///
    /// # Errors
    /// Returns a [`NameError`] when the path cannot be joined.
    pub fn developer_dir_file(&self) -> Result<BoxPath, NameError> {
        BoxPath::under(self.launch.as_str(), "developer-dir")
    }

    /// The Xcode build settings every Task build is forced to use.
    #[must_use]
    pub fn xcconfig_text(&self) -> String {
        format!(
            "HOME = {}\n\
             OTHER_SWIFT_FLAGS = $(inherited) -disable-sandbox\n\
             ONLY_ACTIVE_ARCH = YES\n\
             CODE_SIGNING_ALLOWED = NO\n\
             CODE_SIGNING_REQUIRED = NO\n\
             CODE_SIGN_IDENTITY =\n",
            self.home.as_str()
        )
    }

    /// `<launch>/bin`, the directory of the `xcodebuild` and `xcrun` shims.
    ///
    /// # Errors
    /// Returns a [`NameError`] when the path cannot be joined.
    pub fn bin_dir(&self) -> Result<BoxPath, NameError> {
        BoxPath::under(self.launch.as_str(), "bin")
    }

    /// The `xcodebuild` shim: the real binary with `SwiftPM`'s nested sandboxes off.
    ///
    /// macOS refuses a sandbox inside the outer Seatbelt, so the manifest and
    /// plugin sandboxes must be disabled. The shim execs the recorded Xcode by
    /// absolute path, so it never calls itself even when first on `PATH`.
    ///
    /// # Errors
    /// Returns a [`NameError`] when the developer dir path cannot be joined.
    pub fn xcodebuild_shim_text(&self) -> Result<String, NameError> {
        let file = self.developer_dir_file()?;
        let file = file.as_str();
        let q = quote(file);
        Ok(format!(
            "#!/bin/sh\n\
             d=$(cat {q}) || exit 1\n\
             [ -n \"$d\" ] || {{ echo \"moor: no Xcode developer dir is recorded in {file}\" >&2; exit 1; }}\n\
             exec \"$d/usr/bin/xcodebuild\" -IDEPackageSupportDisableManifestSandbox=YES -IDEPackageSupportDisablePluginExecutionSandbox=YES \"$@\"\n"
        ))
    }

    /// The `xcrun` shim: `xcrun xcodebuild` goes to the xcodebuild shim.
    ///
    /// `xcrun` does not search `PATH` for `xcodebuild`, so Flutter's internal
    /// `xcrun xcodebuild` call would skip the shim without this.
    ///
    /// # Errors
    /// Returns a [`NameError`] when the shim path cannot be joined.
    pub fn xcrun_shim_text(&self) -> Result<String, NameError> {
        let shim = quote(BoxPath::under(self.bin_dir()?.as_str(), "xcodebuild")?.as_str());
        Ok(format!(
            "#!/bin/sh\n\
             if [ \"$1\" = xcodebuild ]; then\n\
             shift\n\
             exec {shim} \"$@\"\n\
             fi\n\
             exec /usr/bin/xcrun \"$@\"\n"
        ))
    }

    /// One idempotent `sh` script that prepares the Task directories and launch files.
    ///
    /// The xcconfig and both shims are rewritten on every run. The developer dir is read from
    /// `xcode-select -p` once and never rewritten, so a later global
    /// `xcode-select` change cannot move a running Task.
    ///
    /// # Errors
    /// Returns a [`NameError`] when a launch file path cannot be joined.
    pub fn prepare_script(&self) -> Result<String, NameError> {
        let home = self.home.as_str();
        let launch = quote(self.launch.as_str());
        let xcconfig = quote(self.xcconfig_file()?.as_str());
        let developer = quote(self.developer_dir_file()?.as_str());
        let dirs = [
            format!("{home}/Library/Caches"),
            format!("{home}/Library/Preferences"),
            format!("{home}/Library/Developer"),
            self.tmp.as_str().to_owned(),
            self.cache.as_str().to_owned(),
            self.sim.as_str().to_owned(),
            self.launch.as_str().to_owned(),
            self.bin_dir()?.as_str().to_owned(),
        ]
        .map(|d| quote(&d))
        .join(" ");
        let bin = self.bin_dir()?;
        let bin_q = quote(bin.as_str());
        let xcodebuild = quote(BoxPath::under(bin.as_str(), "xcodebuild")?.as_str());
        let xcrun = quote(BoxPath::under(bin.as_str(), "xcrun")?.as_str());
        let xcodebuild_text = quote(&self.xcodebuild_shim_text()?);
        let xcrun_text = quote(&self.xcrun_shim_text()?);
        let private = [home, self.tmp.as_str(), self.launch.as_str()]
            .map(quote)
            .join(" ");
        let text = quote(&self.xcconfig_text());
        Ok(format!(
            "set -e\n\
             mkdir -p {dirs}\n\
             chmod 700 {private}\n\
             t=$(mktemp {launch}/.xcconfig.XXXXXX)\n\
             printf '%s' {text} > \"$t\"\n\
             chmod 0444 \"$t\"\n\
             mv -f \"$t\" {xcconfig}\n\
             t=$(mktemp {bin_q}/.xcodebuild.XXXXXX)\n\
             printf '%s' {xcodebuild_text} > \"$t\"\n\
             chmod 0555 \"$t\"\n\
             mv -f \"$t\" {xcodebuild}\n\
             t=$(mktemp {bin_q}/.xcrun.XXXXXX)\n\
             printf '%s' {xcrun_text} > \"$t\"\n\
             chmod 0555 \"$t\"\n\
             mv -f \"$t\" {xcrun}\n\
             if [ ! -s {developer} ]; then\n\
             t=$(mktemp {launch}/.developer-dir.XXXXXX)\n\
             if ! xcode-select -p > \"$t\" || [ ! -s \"$t\" ]; then rm -f \"$t\"; exit 1; fi\n\
             chmod 0444 \"$t\"\n\
             mv -f \"$t\" {developer}\n\
             fi\n"
        ))
    }
}

#[cfg(test)]
mod tests;
