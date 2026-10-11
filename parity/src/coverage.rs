use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Code {
    #[serde(rename = "LIVE")]
    Live,
    #[serde(rename = "KNOWN-BUG")]
    KnownBug,
    #[serde(rename = "SMELL-REDESIGNED")]
    SmellRedesigned,
    #[serde(rename = "DRIVER-SIDE")]
    DriverSide,
    #[serde(rename = "DROPPED")]
    Dropped,
    #[serde(rename = "DOC")]
    Doc,
}

impl Code {
    pub fn as_str(&self) -> &'static str {
        match self {
            Code::Live => "LIVE",
            Code::KnownBug => "KNOWN-BUG",
            Code::SmellRedesigned => "SMELL-REDESIGNED",
            Code::DriverSide => "DRIVER-SIDE",
            Code::Dropped => "DROPPED",
            Code::Doc => "DOC",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exclusion {
    pub code: Code,
    pub reason: String,
    pub r#ref: String,
}

#[derive(Debug, Clone, Default)]
pub struct Inventory {
    pub ids: BTreeSet<String>,
    pub security: BTreeSet<String>,
}

const PREFIXES: [&str; 5] = ["A", "B", "C", "D", "REQ"];
const FILES: [&str; 4] = [
    "a-task-lifecycle.md",
    "b-box-driver.md",
    "c-credentials-env.md",
    "d-reports-contract-lessons.md",
];

fn leading_id(s: &str) -> Option<String> {
    let end = s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))?;
    let id = &s[..end];
    let (prefix, num) = id.rsplit_once('-')?;
    let digits = num
        .strip_suffix(|c: char| c.is_ascii_lowercase())
        .unwrap_or(num);
    let ok = PREFIXES.contains(&prefix)
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| id.to_string())
}

fn looks_like_id(s: &str) -> bool {
    PREFIXES
        .iter()
        .any(|p| s.strip_prefix(p).is_some_and(|rest| rest.starts_with('-')))
}

fn parse_inventory_text(text: &str, inv: &mut Inventory) -> Result<(), String> {
    let mut in_flag_list = false;
    for line in text.lines() {
        if line.starts_with("## ") {
            in_flag_list = false;
        }
        if in_flag_list
            && !line.is_empty()
            && !line.starts_with("- ")
            && !line.starts_with(char::is_whitespace)
        {
            in_flag_list = false;
        }
        if line.starts_with("SECURITY-INVARIANT rows (each") {
            in_flag_list = true;
        }
        if let Some(rest) = line.strip_prefix("| ") {
            match leading_id(rest) {
                Some(id) => {
                    if line.contains("SECURITY-INVARIANT") {
                        inv.security.insert(id.clone());
                    }
                    inv.ids.insert(id);
                }
                None if looks_like_id(rest) => {
                    return Err(format!("malformed inventory id row: {line}"));
                }
                None => {}
            }
        } else if in_flag_list {
            if let Some(id) = line.strip_prefix("- ").and_then(leading_id) {
                inv.security.insert(id);
            }
        }
    }
    Ok(())
}

pub fn parse_inventory(dir: &Path) -> Result<Inventory, String> {
    let mut inv = Inventory::default();
    for name in FILES {
        let path = dir.join(name);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        parse_inventory_text(&text, &mut inv).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(inv)
}

pub fn parse_exclusions(text: &str) -> Result<BTreeMap<String, Exclusion>, String> {
    toml::from_str(text).map_err(|e| format!("exclusions: {e}"))
}

pub fn load_exclusions(path: &Path) -> Result<BTreeMap<String, Exclusion>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    parse_exclusions(&text)
}

#[derive(Debug, Clone)]
pub struct Summary {
    pub covered: usize,
    pub counts: BTreeMap<&'static str, usize>,
}

impl Summary {
    pub fn render(&self) -> String {
        let mut out = format!("covered {}\n", self.covered);
        for (code, n) in &self.counts {
            out.push_str(&format!("{code} {n}\n"));
        }
        out
    }
}

pub fn check(
    inv: &Inventory,
    excl: &BTreeMap<String, Exclusion>,
    covers: &BTreeSet<String>,
) -> Result<Summary, Vec<String>> {
    let mut errs = Vec::new();
    for id in &inv.ids {
        let covered = covers.contains(id);
        let excluded = excl.contains_key(id);
        if !covered && !excluded {
            errs.push(format!("{id}: missing (neither covered nor excluded)"));
        }
        if covered && excluded {
            errs.push(format!("{id}: both covered and excluded"));
        }
    }
    for id in covers.iter().filter(|id| !inv.ids.contains(*id)) {
        errs.push(format!("{id}: unknown covered id (not in inventory)"));
    }
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (id, e) in excl {
        if !inv.ids.contains(id) {
            errs.push(format!("{id}: unknown excluded id (not in inventory)"));
            continue;
        }
        *counts.entry(e.code.as_str()).or_default() += 1;
        if inv.security.contains(id) {
            errs.push(format!(
                "{id}: SECURITY-INVARIANT row excluded as {}; must be covered",
                e.code.as_str()
            ));
        }
        if e.reason.trim().is_empty() {
            errs.push(format!("{id}: empty reason"));
        }
        if e.r#ref.trim().is_empty() {
            errs.push(format!("{id}: empty ref"));
        }
    }
    if errs.is_empty() {
        Ok(Summary {
            covered: covers.len(),
            counts,
        })
    } else {
        Err(errs)
    }
}

const LIVE_REF_PREFIX: &str = "live-smoke.md: ";

pub fn parse_live_steps(text: &str) -> BTreeMap<String, String> {
    let mut steps: BTreeMap<String, String> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let heading = line
            .strip_prefix("### ")
            .or_else(|| line.strip_prefix("## "));
        if let Some(h) = heading {
            let name = h.trim().to_string();
            steps.entry(name.clone()).or_default();
            current = Some(name);
        } else if let Some(body) = current.as_ref().and_then(|c| steps.get_mut(c)) {
            body.push_str(line);
            body.push('\n');
        }
    }
    steps
}

fn lists_row(body: &str, id: &str) -> bool {
    body.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .any(|tok| tok == id)
}

pub fn check_live_refs(
    excl: &BTreeMap<String, Exclusion>,
    steps: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut errs = Vec::new();
    for (id, e) in excl.iter().filter(|(_, e)| e.code == Code::Live) {
        let Some(step) = e.r#ref.strip_prefix(LIVE_REF_PREFIX) else {
            errs.push(format!(
                "{id}: LIVE ref must start with {LIVE_REF_PREFIX:?}"
            ));
            continue;
        };
        match steps.get(step.trim()) {
            None => errs.push(format!("{id}: live-smoke.md has no step {step:?}")),
            Some(body) if !lists_row(body, id) => {
                errs.push(format!("{id}: step {step:?} does not list the row"))
            }
            Some(_) => {}
        }
    }
    errs
}

pub fn live_smoke_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("live-smoke.md")
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("manifest dir has a parent")
        .to_path_buf()
}

pub fn inventory_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("inventory")
}

pub fn exclusions_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("exclusions.toml")
}

#[cfg(test)]
mod tests;
