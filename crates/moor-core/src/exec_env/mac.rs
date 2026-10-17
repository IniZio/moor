//! The per-Task environment of a macOS Box, derived from the Task layout so Tasks do not share the
//! login home, temp directory, tool caches or Xcode configuration.

use crate::boxscript::quote;
use crate::name::BoxPath;
use crate::task_key::{MacLayout, TaskKey};

/// One Task's Mac Box: its directories, its key and the credential nonce of this run.
#[derive(Debug, Clone, Copy)]
pub struct MacTask<'a> {
    /// The Task's directories.
    pub layout: &'a MacLayout,
    /// The key those directories derive from.
    pub key: &'a TaskKey,
    /// The nonce that makes this run's Box tag unique.
    pub nonce: &'a str,
    /// Environment of the selected host toolchains; a name the Task sets is dropped.
    pub toolchain_env: &'a [(String, String)],
    /// Absolute Box directories of the selected toolchains' executables, in `PATH` order (before Homebrew).
    pub toolchain_bin: &'a [String],
    /// The Task's simulator device, when it has one.
    pub sim_udid: Option<&'a str>,
}

/// `(name, directory, relative path)` for the variables that point into the Task directories.
const fn rooted<'a>(m: &'a MacTask<'_>) -> [(&'static str, &'a BoxPath, &'static str); 12] {
    let (home, cache) = (&m.layout.home, &m.layout.cache);
    [
        ("YARN_CACHE_FOLDER", cache, "yarn"),
        ("YARN_GLOBAL_FOLDER", cache, "yarn-global"),
        ("npm_config_cache", cache, "npm"),
        ("COREPACK_HOME", cache, "corepack"),
        ("CP_HOME_DIR", cache, "cocoapods"),
        ("CP_CACHE_DIR", cache, "cocoapods/cache"),
        // GEM_PATH is left unset so Ruby keeps its own gem directories.
        ("GEM_HOME", cache, "gem"),
        ("GEM_SPEC_CACHE", cache, "gem/specs"),
        ("PUB_CACHE", cache, "pub"),
        ("XDG_CACHE_HOME", cache, "xdg"),
        ("XDG_CONFIG_HOME", home, ".config"),
        ("XDG_DATA_HOME", home, ".local/share"),
    ]
}

/// The Task's variables.
fn vars(m: &MacTask<'_>) -> Result<Vec<(String, String)>, String> {
    let l = m.layout;
    let mut out = vec![
        ("HOME".to_owned(), l.home.as_str().to_owned()),
        ("CFFIXED_USER_HOME".to_owned(), l.home.as_str().to_owned()),
        ("TMPDIR".to_owned(), l.tmp.as_str().to_owned()),
        (
            "XCODE_XCCONFIG_FILE".to_owned(),
            l.xcconfig_file()
                .map_err(|e| e.to_string())?
                .as_str()
                .to_owned(),
        ),
        (
            "MOOR_BOX_TAG".to_owned(),
            format!("{}-{}", m.key.as_str(), m.nonce),
        ),
    ];
    if let Some(udid) = m.sim_udid {
        out.push(("MOOR_SIM_UDID".to_owned(), udid.to_owned()));
        out.push(("MOOR_SIM_SET".to_owned(), l.sim.as_str().to_owned()));
    }
    for (name, dir, rel) in &rooted(m) {
        let p = BoxPath::under(dir.as_str(), rel).map_err(|e| e.to_string())?;
        out.push(((*name).to_owned(), p.as_str().to_owned()));
    }
    for (name, value) in m.toolchain_env {
        if !out.iter().any(|(n, _)| n == name) {
            out.push((name.clone(), value.clone()));
        }
    }
    Ok(out)
}

/// `env` without the names the Task sets, then the Task's variables; none changes `env`.
///
/// # Errors
/// Returns a message when a path under the layout is invalid.
pub(super) fn overlay(
    env: Vec<(String, String)>,
    m: Option<&MacTask<'_>>,
) -> Result<Vec<(String, String)>, String> {
    let Some(m) = m else { return Ok(env) };
    let task = vars(m)?;
    let mut out: Vec<(String, String)> = env
        .into_iter()
        .filter(|(n, _)| !task.iter().any(|(t, _)| t == n))
        .collect();
    out.extend(task);
    Ok(out)
}

/// The prelude lines that restore the `DEVELOPER_DIR` recorded for this Task, when one was recorded,
/// and put the Task's shims directory, the selected toolchain directories and Homebrew first on `PATH`,
/// in that order, so a toolchain the owner selected wins over Homebrew.
///
/// # Errors
/// Returns a message when the file path is invalid.
pub(super) fn prelude(m: Option<&MacTask<'_>>) -> Result<Vec<String>, String> {
    let Some(m) = m else { return Ok(Vec::new()) };
    let file = m.layout.developer_dir_file().map_err(|e| e.to_string())?;
    let f = quote(file.as_str());
    let bin = quote(m.layout.bin_dir().map_err(|e| e.to_string())?.as_str());
    let tools = m
        .toolchain_bin
        .iter()
        .fold(String::new(), |acc, d| acc + ":" + &quote(d));
    Ok(vec![
        format!("if [ -s {f} ]; then DEVELOPER_DIR=$(cat {f}); export DEVELOPER_DIR; fi"),
        format!("PATH={bin}{tools}:/opt/homebrew/bin:/opt/homebrew/sbin:\"$PATH\"; export PATH"),
    ])
}

#[cfg(test)]
mod tests;
