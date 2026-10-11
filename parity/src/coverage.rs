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
    #[serde(rename = "PENDING")]
    Pending,
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
            Code::Pending => "PENDING",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exclusion {
    pub code: Code,
    pub reason: String,
    pub r#ref: String,
    #[serde(default)]
    pub slice: Option<String>,
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
    let ok =
        PREFIXES.contains(&prefix) && !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| id.to_string())
}

fn parse_inventory_text(text: &str, inv: &mut Inventory) {
    let mut in_flag_list = false;
    for line in text.lines() {
        if line.starts_with("## ") {
            in_flag_list = false;
        }
        if line.starts_with("SECURITY-INVARIANT rows (each") {
            in_flag_list = true;
        }
        if let Some(rest) = line.strip_prefix("| ") {
            if let Some(id) = leading_id(rest) {
                if line.contains("SECURITY-INVARIANT") {
                    inv.security.insert(id.clone());
                }
                inv.ids.insert(id);
            }
        } else if in_flag_list {
            if let Some(id) = line.strip_prefix("- ").and_then(leading_id) {
                inv.security.insert(id);
            }
        }
    }
}

pub fn parse_inventory(dir: &Path) -> Result<Inventory, String> {
    let mut inv = Inventory::default();
    for name in FILES {
        let path = dir.join(name);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        parse_inventory_text(&text, &mut inv);
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

fn slice_ok(s: &str) -> bool {
    s.strip_prefix('T')
        .and_then(|n| n.parse::<u32>().ok())
        .is_some_and(|n| (4..=10).contains(&n))
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
        if inv.security.contains(id) && e.code != Code::Pending {
            errs.push(format!(
                "{id}: SECURITY-INVARIANT row excluded as {}; only PENDING allowed",
                e.code.as_str()
            ));
        }
        match (&e.code, &e.slice) {
            (Code::Pending, None) => errs.push(format!("{id}: PENDING requires slice")),
            (Code::Pending, Some(s)) if !slice_ok(s) => {
                errs.push(format!("{id}: slice {s:?} outside T4..T10"))
            }
            (Code::Pending, Some(_)) | (_, None) => {}
            (_, Some(_)) => errs.push(format!("{id}: slice only allowed with PENDING")),
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

    fn entry(id: &str, code: &str, slice: Option<&str>) -> String {
        let slice = slice
            .map(|s| format!("slice = \"{s}\"\n"))
            .unwrap_or_default();
        format!("[{id}]\ncode = \"{code}\"\n{slice}reason = \"r\"\nref = \"p\"\n")
    }

    fn errs(e: &str, covers: &[&str]) -> Vec<String> {
        check(&inv(), &excl(e), &set(covers)).unwrap_err()
    }

    #[test]
    fn missing_id_fails() {
        let e = errs(&entry("A-01", "DOC", None), &["C-03"]);
        assert!(e
            .iter()
            .any(|m| m.starts_with("A-02") && m.contains("missing")));
    }

    #[test]
    fn security_row_excluded_as_doc_fails() {
        let body = entry("C-03", "DOC", None);
        let e = errs(&body, &["A-01", "A-02"]);
        assert!(e
            .iter()
            .any(|m| m.starts_with("C-03") && m.contains("SECURITY")));
    }

    #[test]
    fn pending_security_row_passes() {
        let body = entry("C-03", "PENDING", Some("T5"));
        let s = check(&inv(), &excl(&body), &set(&["A-01", "A-02"])).unwrap();
        assert_eq!(s.render(), "covered 2\nPENDING 1\n");
    }

    #[test]
    fn pending_slice_rules() {
        let none = errs(&entry("A-01", "PENDING", None), &["A-02", "C-03"]);
        assert!(none.iter().any(|m| m.contains("requires slice")));
        let t3 = errs(&entry("A-01", "PENDING", Some("T3")), &["A-02", "C-03"]);
        assert!(t3.iter().any(|m| m.contains("outside")));
        let doc = errs(&entry("A-01", "DOC", Some("T4")), &["A-02", "C-03"]);
        assert!(doc.iter().any(|m| m.contains("only allowed with PENDING")));
    }

    #[test]
    fn id_in_both_fails() {
        let e = errs(&entry("A-01", "DOC", None), &["A-01", "A-02", "C-03"]);
        assert!(e.iter().any(|m| m.contains("both")));
    }

    #[test]
    fn unknown_ids_fail() {
        let body = entry("Z-9", "DOC", None);
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
        assert!(parse_exclusions(&entry("A-01", "NOPE", None)).is_err());
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
}
