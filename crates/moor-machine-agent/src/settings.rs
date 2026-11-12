//! The settings warm starts need in the crabbox user config.
//!
//! `capabilities.checkpoints` gives crabbox the verbs, and `config.checkpoints` tells the adapter
//! to make its `doctor` check for a hypervisor that can checkpoint. Each is added once when
//! missing. A value the owner set stays as set, so `false` is how an owner opts out. The adapter
//! already builds its boot template unless told not to.
//!
//! Each setting is a (section, key, value) entry that `ensure` adds once when missing. The
//! Boxes slice is one more entry, `config.cgroupSlice`. An owner's slice stays, so an owner can
//! point Boxes at another slice. With the adapter's `cgroup: off` the slice is not added, since
//! the adapter rejects the pair.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// The crabbox provider whose user config section this concerns.
const PROVIDER: &str = "external";

/// One key of the provider's config, added when missing.
struct Setting<'a> {
    /// The mapping under the provider that holds it.
    section: &'a str,
    /// Its name.
    key: &'a str,
    /// What it is set to.
    value: Value,
}

/// The checkpoint entries, the last written first in the file.
fn checkpoints() -> [Setting<'static>; 2] {
    ["config", "capabilities"].map(|section| Setting {
        section,
        key: "checkpoints",
        value: Value::Bool(true),
    })
}

/// What [`ensure`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ensured {
    /// A key was missing and is now there.
    Added,
    /// The keys are set already, or the provider has no such setting.
    Kept,
    /// The owner's config has `cgroup: off`, so Boxes get no memory ceiling and no slice was added.
    NoCeiling,
}

/// The crabbox user config under `home`, where crabbox itself looks for it.
#[must_use]
pub fn config_path(home: &Path) -> PathBuf {
    if let Some(explicit) = std::env::var_os("CRABBOX_CONFIG").filter(|p| !p.is_empty()) {
        return PathBuf::from(explicit);
    }
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|p| !p.is_empty())
        .map_or_else(|| default_config_dir(home), PathBuf::from);
    dir.join("crabbox").join("config.yaml")
}

/// Where the platform keeps a user's config under `home`.
fn default_config_dir(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        home.join(".config")
    }
}

/// The key by which the config at `path` says no to checkpoints on `provider`, if it does. With
/// the adapter's own key off, `doctor` no longer checks the hypervisor, so that counts as well.
#[must_use]
pub fn opted_out(path: &Path, provider: &str) -> Option<&'static str> {
    if provider != PROVIDER {
        return None;
    }
    let config = parse(&fs::read_to_string(path).ok()?).ok()?;
    let says_no = |pointer: &str| config.pointer(pointer) == Some(&Value::Bool(false));
    if says_no("/external/capabilities/checkpoints") {
        Some("capabilities.checkpoints")
    } else if says_no("/external/config/checkpoints") {
        Some("config.checkpoints")
    } else {
        None
    }
}

/// Adds each missing `checkpoints: true` to the config at `path`, for a `provider` that reads it.
///
/// The keys go under `external.capabilities` and `external.config`. The rest of the file stays, and
/// the file is replaced atomically.
///
/// # Errors
/// The file cannot be read, parsed or written, or holds something other than a mapping on the
/// way to the key. The file is then left as it was.
pub fn ensure(path: &Path, provider: &str) -> Result<Ensured, String> {
    ensure_with(path, provider, &checkpoints())
}

/// Adds `external.config.cgroupSlice: slice` to the config at `path` when missing, so the adapter
/// puts each VM's scope in it. The caller has written the slice's unit first.
///
/// # Errors
/// As [`ensure`].
pub fn ensure_slice(path: &Path, provider: &str, slice: &str) -> Result<Ensured, String> {
    if provider == PROVIDER && cgroup_off(path) {
        return Ok(Ensured::NoCeiling);
    }
    let setting = Setting {
        section: "config",
        key: "cgroupSlice",
        value: Value::String(slice.to_owned()),
    };
    ensure_with(path, provider, &[setting])
}

/// Whether the config at `path` turns the adapter's cgroups off, spelled `"off"` or as YAML 1.1's
/// bare `off`.
fn cgroup_off(path: &Path) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| parse(&text).ok())
        .is_some_and(|c| match c.pointer("/external/config/cgroup") {
            Some(Value::String(mode)) => mode == "off",
            Some(Value::Bool(on)) => !on,
            _ => false,
        })
}

/// Adds each of `settings` missing from the config at `path`.
fn ensure_with(path: &Path, provider: &str, settings: &[Setting<'_>]) -> Result<Ensured, String> {
    if provider != PROVIDER {
        return Ok(Ensured::Kept);
    }
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let fail = |e: String| format!("{}: {e}", path.display());
    let mut value = parse(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))?;
    let mut out = text.clone();
    for setting in settings {
        let Some(want) = with_key(&value, setting).map_err(fail)? else {
            continue;
        };
        let spliced = splice(&out, setting);
        out = if parse(&spliced).is_ok_and(|v| v == want) {
            spliced
        } else {
            serde_saphyr::to_string(&want).map_err(|e| e.to_string())?
        };
        value = want;
    }
    if out == text {
        return Ok(Ensured::Kept);
    }
    write_atomically(path, &out)?;
    Ok(Ensured::Added)
}

/// The value of a YAML text; an empty or comment-only file is an empty mapping.
fn parse(text: &str) -> Result<Value, String> {
    let value: Option<Value> = serde_saphyr::from_str(text).map_err(|e| e.to_string())?;
    Ok(value
        .filter(|v| !v.is_null())
        .unwrap_or_else(|| Value::Object(Map::new())))
}

/// `was` with `setting` added; none when it has the key.
fn with_key(was: &Value, setting: &Setting<'_>) -> Result<Option<Value>, String> {
    let section = setting.section;
    let mut want = was.clone();
    let mut at = &mut want;
    for name in [PROVIDER, section] {
        let Value::Object(map) = at else {
            return Err(format!(
                "{name} cannot be added under a value that is no mapping"
            ));
        };
        let next = map.entry(name).or_insert(Value::Null);
        if next.is_null() {
            *next = Value::Object(Map::new());
        }
        at = next;
    }
    let Value::Object(keys) = at else {
        return Err(format!("{section} holds a value that is no mapping"));
    };
    if keys.contains_key(setting.key) {
        return Ok(None);
    }
    keys.insert(setting.key.to_owned(), setting.value.clone());
    Ok(Some(want))
}

/// `text` with `setting` written in by line, so its comments and order stay. It covers the shapes
/// people write; [`ensure`] checks the result means what it should before using it.
fn splice(text: &str, setting: &Setting<'_>) -> String {
    let section_name = setting.section;
    let entry = format!("{}: {}", setting.key, setting.value);
    let lines: Vec<&str> = text.lines().collect();
    let Some(section) = lines.iter().position(|l| is_key(l, "external")) else {
        let sep = if text.is_empty() || text.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        return format!("{text}{sep}external:\n  {section_name}:\n    {entry}\n");
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(section + 1)
        .find(|(_, l)| !l.trim().is_empty() && !l.starts_with([' ', '\t', '#']))
        .map_or(lines.len(), |(i, _)| i);
    let body = lines.get(section + 1..end).unwrap_or_default();
    let direct = indent_of(body).unwrap_or_default();
    let label = format!("{direct}{section_name}:");
    let found = body.iter().position(|l| l.starts_with(&label));
    let (at, new) = match found {
        None => {
            let indent = indent_of(body).unwrap_or_else(|| "  ".to_owned());
            (
                section + 1,
                format!("{indent}{section_name}:\n{indent}  {entry}"),
            )
        }
        Some(i) => {
            let line = body.get(i).copied().unwrap_or_default();
            if line.contains('{') {
                return replace_line(
                    &lines,
                    section + 1 + i,
                    &line.replacen('{', &format!("{{ {entry},"), 1),
                );
            }
            let below = body.get(i + 1..).unwrap_or_default();
            let own = line.len() - line.trim_start().len();
            let indent = indent_of(below)
                .filter(|child| child.len() > own)
                .unwrap_or_else(|| format!("{}  ", " ".repeat(own)));
            (section + 2 + i, format!("{indent}{entry}"))
        }
    };
    let mut out: Vec<&str> = lines.clone();
    out.insert(at, &new);
    out.join("\n") + "\n"
}

/// Whether `line` starts the top-level block mapping `key`, its value below it.
fn is_key(line: &str, key: &str) -> bool {
    line.strip_prefix(key)
        .and_then(|rest| rest.strip_prefix(':'))
        .is_some_and(|rest| {
            let rest = rest.trim();
            rest.is_empty() || rest.starts_with('#')
        })
}

/// The indent of the first line in `lines` that holds a key.
fn indent_of(lines: &[&str]) -> Option<String> {
    lines
        .iter()
        .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .map(|l| l.chars().take_while(|c| *c == ' ').collect())
}

/// `lines` with line `at` replaced.
fn replace_line(lines: &[&str], at: usize, new: &str) -> String {
    let mut out = lines.to_vec();
    if let Some(slot) = out.get_mut(at) {
        *slot = new;
    }
    out.join("\n") + "\n"
}

/// Replaces the file at `path`, through a symlink if it is one, with `text`; a new file is the
/// user's alone, an existing one keeps its mode.
fn write_atomically(path: &Path, text: &str) -> Result<(), String> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    let fail = |e: std::io::Error| format!("cannot write {}: {e}", target.display());
    // crabbox refuses to read its routing file under a directory others can write.
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(fail)?;
    let mode = fs::metadata(&target).map_or(0o600, |m| m.permissions().mode());
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(fail)?;
    tmp.write_all(text.as_bytes()).map_err(fail)?;
    tmp.as_file()
        .set_permissions(fs::Permissions::from_mode(mode))
        .map_err(fail)?;
    tmp.as_file().sync_all().map_err(fail)?;
    tmp.persist(&target).map_err(|e| fail(e.error))?;
    Ok(())
}
