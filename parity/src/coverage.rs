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
    repo_root().join(".groundwork/work/v2/live-smoke.md")
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("manifest dir has a parent")
        .to_path_buf()
}

pub fn inventory_dir() -> PathBuf {
    repo_root().join(".groundwork/work/v2/inventory")
}

pub fn exclusions_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("exclusions.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inv() -> Inventory {
        Inventory {
            ids: ["A-01", "A-02", "C-03"].map(String::from).into(),
            security: ["C-03"].map(String::from).into(),
        }
    }

    fn set(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    fn excl(body: &str) -> BTreeMap<String, Exclusion> {
        parse_exclusions(body).unwrap()
    }

    fn entry(id: &str, code: &str) -> String {
        format!("[{id}]\ncode = \"{code}\"\nreason = \"r\"\nref = \"p\"\n")
    }

    fn errs(e: &str, covers: &[&str]) -> Vec<String> {
        check(&inv(), &excl(e), &set(covers)).unwrap_err()
    }

    #[test]
    fn missing_id_fails() {
        let e = errs(&entry("A-01", "DOC"), &["C-03"]);
        assert!(e
            .iter()
            .any(|m| m.starts_with("A-02") && m.contains("missing")));
    }

    #[test]
    fn security_row_excluded_as_doc_fails() {
        let body = entry("C-03", "DOC");
        let e = errs(&body, &["A-01", "A-02"]);
        assert!(e
            .iter()
            .any(|m| m.starts_with("C-03") && m.contains("SECURITY")));
    }

    #[test]
    fn slice_field_is_rejected() {
        let body = "[A-01]\ncode = \"DOC\"\nslice = \"T4\"\nreason = \"r\"\nref = \"p\"\n";
        assert!(parse_exclusions(body).is_err());
    }

    #[test]
    fn pending_code_is_rejected() {
        assert!(parse_exclusions(&entry("A-01", "PENDING")).is_err());
    }

    #[test]
    fn id_in_both_fails() {
        let e = errs(&entry("A-01", "DOC"), &["A-01", "A-02", "C-03"]);
        assert!(e.iter().any(|m| m.contains("both")));
    }

    #[test]
    fn unknown_ids_fail() {
        let body = entry("Z-9", "DOC");
        let e = errs(&body, &["A-01", "A-02", "C-03", "B-77"]);
        assert!(e
            .iter()
            .any(|m| m.starts_with("Z-9") && m.contains("unknown")));
        assert!(e
            .iter()
            .any(|m| m.starts_with("B-77") && m.contains("unknown")));
    }

    #[test]
    fn rejects_unknown_field_and_code() {
        assert!(parse_exclusions("[A-01]\ncode=\"DOC\"\nreason=\"r\"\nref=\"p\"\nx=1\n").is_err());
        assert!(parse_exclusions(&entry("A-01", "NOPE")).is_err());
    }

    #[test]
    fn empty_reason_fails() {
        let body = "[A-01]\ncode = \"DOC\"\nreason = \"\"\nref = \"p\"\n";
        assert!(errs(body, &["A-02", "C-03"])
            .iter()
            .any(|m| m.contains("empty reason")));
    }

    #[test]
    fn real_inventory_parses() {
        let inv = parse_inventory(&inventory_dir()).unwrap();
        for p in PREFIXES {
            let pre = format!("{p}-");
            assert!(inv.ids.iter().any(|i| i.starts_with(&pre)), "no {p} ids");
        }
        assert!(!inv.security.is_empty());
        assert!(inv.security.is_subset(&inv.ids));
    }

    #[test]
    fn real_inventory_counts() {
        let inv = parse_inventory(&inventory_dir()).unwrap();
        let mut got = BTreeMap::new();
        for id in &inv.ids {
            let (p, _) = id.rsplit_once('-').unwrap();
            *got.entry(p.to_string()).or_insert(0usize) += 1;
        }
        println!(
            "counts {got:?} total {} security {}",
            inv.ids.len(),
            inv.security.len()
        );
        let want: BTreeMap<String, usize> =
            [("A", 90), ("B", 108), ("C", 141), ("D", 43), ("REQ", 52)]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect();
        assert_eq!(got, want);
        assert_eq!(inv.ids.len(), 434);
        assert_eq!(inv.security.len(), 52);
    }

    const SMOKE: &str =
        "# Live\n\n## host Task\n\nCovers A-42 and B-37.\n\n### egress block\n\nB-102a\n";

    fn live(id: &str, r: &str) -> BTreeMap<String, Exclusion> {
        let body = format!("[{id}]\ncode = \"LIVE\"\nreason = \"r\"\nref = \"{r}\"\n");
        excl(&body)
    }

    #[test]
    fn live_steps_parse_both_heading_levels() {
        let steps = parse_live_steps(SMOKE);
        assert_eq!(
            steps.keys().cloned().collect::<Vec<_>>(),
            ["egress block", "host Task"]
        );
        assert!(steps["host Task"].contains("A-42"));
    }

    #[test]
    fn live_ref_must_name_a_step_listing_the_row() {
        let steps = parse_live_steps(SMOKE);
        assert!(check_live_refs(&live("A-42", "live-smoke.md: host Task"), &steps).is_empty());
        let wrong = check_live_refs(&live("A-42", "live-smoke.md: nope"), &steps);
        assert!(wrong[0].contains("no step"));
        let free = check_live_refs(&live("A-42", "usage-log item 1"), &steps);
        assert!(free[0].contains("must start with"));
        let unlisted = check_live_refs(&live("B-99", "live-smoke.md: host Task"), &steps);
        assert!(unlisted[0].contains("does not list"));
        let not_live = excl(&entry("A-42", "DOC"));
        assert!(check_live_refs(&not_live, &steps).is_empty());
    }

    #[test]
    fn row_listing_needs_whole_id() {
        assert!(lists_row("rows B-37, B-41.", "B-37"));
        assert!(!lists_row("rows B-371", "B-37"));
        assert!(!lists_row("rows B-102a", "B-102"));
    }

    #[test]
    fn letter_suffix_id_parses_and_malformed_row_errors() {
        let mut inv = Inventory::default();
        parse_inventory_text("| B-102a | row |\n", &mut inv).unwrap();
        assert!(inv.ids.contains("B-102a"));
        let err = parse_inventory_text("| B-1x2 | row |\n", &mut Inventory::default());
        assert!(err.unwrap_err().contains("B-1x2"));
    }

    #[test]
    fn flag_list_ends_at_next_label() {
        let text =
            "SECURITY-INVARIANT rows (each x):\n\n- C-01: a\n\nKNOWN-BUG rows:\n\n- C-02: b\n";
        let mut inv = Inventory::default();
        parse_inventory_text(text, &mut inv).unwrap();
        assert_eq!(inv.security, set(&["C-01"]));
    }
}
