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

    /// One idempotent `sh` script that prepares the Task directories and launch files.
    ///
    /// The xcconfig is rewritten on every run. The developer dir is read from
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
        ]
        .map(|d| quote(&d))
        .join(" ");
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
