//! The macOS `Seatbelt` part of the nono profile.
//!
//! This module is the only place that emits `Seatbelt` rule text and the only
//! writer of the profile key `unsafe_macos_seatbelt_rules`. Rule text is built
//! from the closed `&'static str` grants of `moor_core::mac_capability` only,
//! never from config or any other input string.

use moor_core::config::RepoSandbox;
use moor_core::mac_capability::{expand_with, Grant, MacCapability, MacPath, MacRoot};
use moor_core::name::BoxPath;
use moor_core::task_key::{inside, MacLayout, CREDENTIAL_DIRS};
use serde_json::{json, Map, Value};

/// The nono profile key that holds raw `Seatbelt` rules.
pub(crate) const RULES_KEY: &str = "unsafe_macos_seatbelt_rules";

/// What the Mac profile builder needs.
pub(crate) struct MacInput<'a> {
    /// The directories of this Box.
    pub(crate) layout: &'a MacLayout,
    /// The Box user's real home.
    pub(crate) login_home: &'a BoxPath,
    /// The absolute active developer directory.
    pub(crate) developer_dir: &'a str,
    /// The capabilities to grant.
    pub(crate) capabilities: &'a [MacCapability],
    /// Project paths from `box.sandbox`.
    pub(crate) sandbox: &'a RepoSandbox,
    /// Selected host toolchains as (name, realpath of the root on the Box), in selection order.
    pub(crate) toolchains: &'a [(String, String)],
}

/// Whether a path is granted read-only or read-write.
#[derive(Clone, Copy)]
enum Access {
    /// Goes to `filesystem.read`.
    Read,
    /// Goes to `filesystem.allow`, which nono treats as read-write.
    Write,
}

/// Adds the expanded grants of `input` to the nono profile `doc`.
///
/// Read paths go to `filesystem.read`, write paths to `filesystem.allow`;
/// existing entries are kept and none is added twice. The login-home credential
/// directories go to `filesystem.deny`. Preference and Mach
/// grants become entries of `unsafe_macos_seatbelt_rules`, omitted when empty.
///
/// # Errors
/// Returns a message when a project path is outside the workdir, when a
/// resolved path is inside the login home without being a login-home grant,
/// when a read or write grant covers a login-home credential directory, or when a grant text holds a quote, backslash or control character.
pub(crate) fn apply(doc: &mut Value, input: &MacInput) -> Result<(), String> {
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    let mut rules: Vec<String> = Vec::new();
    for g in expand_with(input.capabilities, input.toolchains.len()) {
        match g.grant {
            Grant::Read(p) => push_path(input, p, Access::Read, &mut reads, &mut writes)?,
            Grant::Write(p) => push_path(input, p, Access::Write, &mut reads, &mut writes)?,
            Grant::PrefDomain(d) => rules.push(format!(
                "(allow user-preference-read user-preference-write (preference-domain \"{}\"))",
                plain(d)?
            )),
            Grant::NestedSandboxExec => rules.push(
                "(allow process-exec (with no-sandbox) (literal \"/usr/bin/sandbox-exec\"))"
                    .to_owned(),
            ),
            Grant::MachLookup(n) => {
                rules.push(format!(
                    "(allow mach-lookup (global-name \"{}\"))",
                    plain(n)?
                ));
            }
        }
    }
    project_paths(input, "read", &input.sandbox.read, &mut reads)?;
    project_paths(input, "write", &input.sandbox.write, &mut writes)?;
    let denies = credential_paths(input);
    refuse_credentials(&denies, reads.iter().chain(&writes))?;
    let root = doc
        .as_object_mut()
        .ok_or_else(|| "nono profile is not an object".to_owned())?;
    let fs = section(root, "filesystem")?;
    extend(fs, "read", reads)?;
    extend(fs, "allow", writes)?;
    extend(fs, "deny", denies)?;
    if !rules.is_empty() {
        extend(root, RULES_KEY, rules)?;
    }
    Ok(())
}

/// Returns `text` when it is safe inside a quoted `Seatbelt` string.
fn plain(text: &str) -> Result<&str, String> {
    if text
        .chars()
        .any(|c| c == '"' || c == '\\' || c.is_control())
    {
        return Err(format!(
            "seatbelt grant `{}` holds a forbidden character",
            text.escape_debug()
        ));
    }
    Ok(text)
}

/// Resolves a capability path and files it under the access it asks for.
fn push_path(
    input: &MacInput,
    p: MacPath,
    access: Access,
    reads: &mut Vec<String>,
    writes: &mut Vec<String>,
) -> Result<(), String> {
    let abs = resolve(input, p);
    match p.root {
        MacRoot::LoginHome => {}
        MacRoot::HostToolchain(i) => {
            if matches!(access, Access::Write) {
                return Err("a host toolchain root is never writable".to_owned());
            }
            let name = input
                .toolchains
                .get(usize::from(i))
                .map_or("?", |t| t.0.as_str());
            refuse_host_toolchain(input, &abs)
                .map_err(|why| format!("host toolchain \"{name}\" root {abs} is refused: {why}"))?;
        }
        _ => refuse_login_home(input, &abs)?,
    }
    match access {
        Access::Read => reads.push(abs),
        Access::Write => writes.push(abs),
    }
    Ok(())
}

/// Resolves a [`MacPath`] to an absolute path.
fn resolve(input: &MacInput, p: MacPath) -> String {
    let l = input.layout;
    let base = match p.root {
        MacRoot::Work => l.work.as_str(),
        MacRoot::Home => l.home.as_str(),
        MacRoot::Tmp => l.tmp.as_str(),
        MacRoot::Cache => l.cache.as_str(),
        MacRoot::Sim => l.sim.as_str(),
        MacRoot::Launch => l.launch.as_str(),
        MacRoot::DeveloperDir => input.developer_dir,
        MacRoot::LoginHome => input.login_home.as_str(),
        MacRoot::HostToolchain(i) => input
            .toolchains
            .get(usize::from(i))
            .map_or("", |t| t.1.as_str()),
        MacRoot::System => return p.rel.to_owned(),
    };
    let base = base.trim_end_matches('/');
    if p.rel.is_empty() {
        if base.is_empty() {
            "/".to_owned()
        } else {
            base.to_owned()
        }
    } else {
        format!("{base}/{}", p.rel)
    }
}

/// Refuses a path inside the login home; only login-home grants may be there.
fn refuse_login_home(input: &MacInput, abs: &str) -> Result<(), String> {
    let under_home = BoxPath::restore(abs).is_ok_and(|b| inside(&b, input.login_home));
    if under_home {
        return Err(format!(
            "`{abs}` is inside the login home {}; only login-home grants may be there",
            input.login_home.as_str()
        ));
    }
    Ok(())
}

/// Says why a host toolchain root may not be readable, or `Ok` when it may.
fn refuse_host_toolchain(input: &MacInput, abs: &str) -> Result<(), String> {
    if abs == "/" {
        return Err(format!(
            "it contains the login home {}",
            input.login_home.as_str()
        ));
    }
    let plain_path = plain(abs).is_ok()
        && !abs.contains("//")
        && abs
            .split('/')
            .skip(1)
            .all(|c| !matches!(c, "" | "." | ".."));
    let root = match BoxPath::restore(abs) {
        Ok(b) if plain_path => b,
        _ => return Err("it is not an absolute plain path".to_owned()),
    };
    let home = input.login_home;
    if inside(home, &root) {
        return Err(format!(
            "it is or contains the login home {}",
            home.as_str()
        ));
    }
    let library = format!("{}/Library", home.as_str().trim_end_matches('/'));
    if BoxPath::restore(&library).is_ok_and(|l| inside(&root, &l)) {
        return Err(format!("it is inside {library}"));
    }
    refuse_credentials(&credential_paths(input), std::iter::once(&abs.to_owned()))
        .map_err(|_| credential_reason(input, &root))?;
    let work_root = input.layout.root.as_str().trim_end_matches('/');
    let parent = work_root.rsplit_once('/').map_or("", |(p, _)| p);
    let parent = if parent.is_empty() { "/" } else { parent };
    if BoxPath::restore(parent).is_ok_and(|w| inside(&root, &w) || inside(&w, &root)) {
        return Err(format!("it overlaps the Box work root {parent}"));
    }
    Ok(())
}

/// Names the credential directory a refused toolchain root overlaps.
fn credential_reason(input: &MacInput, root: &BoxPath) -> String {
    for deny in credential_paths(input) {
        let Ok(d) = BoxPath::restore(&deny) else {
            continue;
        };
        if inside(root, &d) {
            return format!("it is inside the credential directory {deny}");
        }
        if inside(&d, root) {
            return format!("it contains the credential directory {deny}");
        }
    }
    "it overlaps a credential directory".to_owned()
}

/// The absolute credential directories of the login home.
fn credential_paths(input: &MacInput) -> Vec<String> {
    let home = input.login_home.as_str().trim_end_matches('/');
    CREDENTIAL_DIRS
        .iter()
        .map(|rel| format!("{home}/{rel}"))
        .collect()
}

/// Refuses a grant that equals, contains or lies inside a denied credential directory.
fn refuse_credentials<'a>(
    denies: &[String],
    grants: impl Iterator<Item = &'a String>,
) -> Result<(), String> {
    let restore = |s: &str| BoxPath::restore(s).ok();
    for grant in grants {
        let Some(g) = restore(grant) else { continue };
        for deny in denies {
            let Some(d) = restore(deny) else { continue };
            if inside(&g, &d) || inside(&d, &g) {
                return Err(format!(
                    "`{grant}` would give the agent access to `{deny}`, which holds login credentials"
                ));
            }
        }
    }
    Ok(())
}

/// Joins `box.sandbox.<kind>` paths onto the workdir and checks they stay inside.
fn project_paths(
    input: &MacInput,
    kind: &str,
    paths: &[String],
    out: &mut Vec<String>,
) -> Result<(), String> {
    for (i, rel) in paths.iter().enumerate() {
        let outside = || format!("box.sandbox.{kind}[{i}] `{rel}` is outside the workdir");
        let joined = BoxPath::under(input.layout.work.as_str(), rel).map_err(|_| outside())?;
        if !inside(&joined, &input.layout.work) {
            return Err(outside());
        }
        refuse_login_home(input, joined.as_str())?;
        out.push(joined.as_str().to_owned());
    }
    Ok(())
}

/// Returns the object at `key`, creating it when missing.
fn section<'a>(
    obj: &'a mut Map<String, Value>,
    key: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    obj.entry(key)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("nono profile `{key}` is not an object"))
}

/// Appends `items` to the string array at `key`, creating it and skipping duplicates.
fn extend(obj: &mut Map<String, Value>, key: &str, items: Vec<String>) -> Result<(), String> {
    let arr = obj
        .entry(key)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| format!("nono profile `{key}` is not an array"))?;
    for item in items {
        if !arr.iter().any(|v| v.as_str() == Some(item.as_str())) {
            arr.push(Value::String(item));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
