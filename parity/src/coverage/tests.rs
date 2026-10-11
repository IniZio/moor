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
    let want: BTreeMap<String, usize> = [("A", 90), ("B", 108), ("C", 141), ("D", 43), ("REQ", 52)]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    assert_eq!(got, want);
    assert_eq!(inv.ids.len(), 434);
    assert_eq!(inv.security.len(), 51);
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
    let text = "SECURITY-INVARIANT rows (each x):\n\n- C-01: a\n\nKNOWN-BUG rows:\n\n- C-02: b\n";
    let mut inv = Inventory::default();
    parse_inventory_text(text, &mut inv).unwrap();
    assert_eq!(inv.security, set(&["C-01"]));
}
