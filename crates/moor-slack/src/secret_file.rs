//! Reading secret files that must be owner-only.

use std::fs::File;
use std::io::{self, Read as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

/// Reads a trimmed secret from `path`, which must be owner-only and non-empty; errors start with `label`.
pub(super) fn read_secret(path: &Path, label: &str) -> Result<String, String> {
    let shown = path.display();
    let fail = |why: &dyn std::fmt::Display| format!("{label} {shown}: {why}");
    let read = || -> io::Result<Result<String, &'static str>> {
        let mut file = File::open(path)?;
        if file.metadata()?.permissions().mode() & 0o077 != 0 {
            return Ok(Err("must be mode 0600"));
        }
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        let token = text.trim_end();
        Ok(if token.is_empty() {
            Err("is empty")
        } else {
            Ok(token.to_owned())
        })
    };
    match read() {
        Ok(Ok(token)) => Ok(token),
        Ok(Err(why)) => Err(fail(&why)),
        Err(e) => Err(fail(&e)),
    }
}

#[cfg(test)]
mod tests;
