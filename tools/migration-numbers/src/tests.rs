use super::*;

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|n| (*n).to_owned()).collect()
}

fn check(base: &[&str], head: &[&str]) -> Vec<String> {
    violations("origin/develop", &names(base), head).unwrap()
}

#[test]
fn a_branch_equal_to_its_base_passes() {
    assert!(check(&["V45__baseline"], &["V45__baseline"]).is_empty());
}

#[test]
fn a_new_step_above_the_base_passes() {
    assert!(check(&["V45__baseline"], &["V45__baseline", "V46__users"]).is_empty());
}

#[test]
fn a_new_step_reusing_a_base_version_names_both() {
    let bad = check(&["V45__baseline", "V46__a"], &["V45__baseline", "V46__b"]);
    assert_eq!(
        bad,
        ["V46__b collides with V46__a on origin/develop: both are version 46"]
    );
}

#[test]
fn a_new_step_below_the_base_highest_fails() {
    let bad = check(
        &["V45__baseline", "V47__a"],
        &["V45__baseline", "V46__b", "V47__a"],
    );
    assert_eq!(bad.len(), 1);
    assert!(
        bad[0].contains("V46__b") && bad[0].contains("V47__a"),
        "{bad:?}"
    );
}

#[test]
fn a_renamed_base_step_fails() {
    let bad = check(&["V45__baseline"], &["V45__squash"]);
    assert!(
        bad[0].contains("V45__squash") && bad[0].contains("V45__baseline"),
        "{bad:?}"
    );
}

#[test]
fn a_version_used_twice_or_out_of_order_fails() {
    assert_eq!(check(&[], &["V1__a", "V1__b"]).len(), 1);
    assert_eq!(check(&[], &["V2__a", "V1__b"]).len(), 1);
}

#[test]
fn a_dropped_base_step_passes_so_a_squash_can_land() {
    assert!(check(&["V1__a", "V2__b"], &["V3__baseline"]).is_empty());
}

#[test]
fn a_malformed_name_is_an_error() {
    assert!(violations("b", &names(&["baseline"]), &[]).is_err());
}

#[test]
fn the_parser_reads_the_list_the_crate_compiles() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(STEPS_FILE),
    )
    .unwrap();
    let compiled: Vec<String> = moor_schema::STEPS
        .iter()
        .map(|s| s.name.to_owned())
        .collect();
    assert_eq!(step_names(&source).unwrap(), compiled);
}

#[test]
fn a_file_without_a_steps_list_is_an_error() {
    assert!(step_names("pub const OTHER: u8 = 1;").is_err());
}
