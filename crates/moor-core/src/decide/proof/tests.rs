use super::*;
use std::process::Command;

fn pf(name: &str, size: u64, sha: &str) -> ProofFile {
    ProofFile {
        name: name.to_owned(),
        size,
        sha256: sha.to_owned(),
    }
}

#[test]
fn parse_keeps_good_lines_sorted() {
    let out = "b.txt\t3\tbb\nsub/a.png\t10\taa\n";
    assert_eq!(
        parse_listing(out),
        vec![pf("b.txt", 3, "bb"), pf("sub/a.png", 10, "aa")]
    );
}

#[test]
fn parse_skips_malformed_lines() {
    let out = "\nnoTabs\nx\tnotnum\th\nx\t1\n\t1\th\nok\t1\th\nx\t1\th\textra\n";
    assert_eq!(parse_listing(out), vec![pf("ok", 1, "h")]);
}

#[test]
fn parse_skips_parent_and_absolute_names() {
    let out = "../x\t1\th\na/../b\t1\th\n/etc/passwd\t1\th\na//b\t1\th\ngood\t1\th\n";
    assert_eq!(parse_listing(out), vec![pf("good", 1, "h")]);
}

#[test]
fn fresh_reports_new_and_changed_only() {
    let listing = [pf("a", 1, "h1"), pf("b", 1, "h2"), pf("c", 1, "h3")];
    let seen = [pf("a", 1, "h1"), pf("b", 1, "old")];
    assert_eq!(
        fresh(&listing, &seen),
        vec![pf("b", 1, "h2"), pf("c", 1, "h3")]
    );
}

#[test]
fn fresh_unchanged_is_empty() {
    let listing = [pf("a", 1, "h1")];
    assert!(fresh(&listing, &listing).is_empty());
}

#[test]
fn fresh_latest_seen_wins() {
    let listing = [pf("a", 1, "h1")];
    let seen = [pf("a", 1, "h1"), pf("a", 1, "h2")];
    assert_eq!(fresh(&listing, &seen), vec![pf("a", 1, "h1")]);
    let seen = [pf("a", 1, "h2"), pf("a", 1, "h1")];
    assert!(fresh(&listing, &seen).is_empty());
}

#[test]
fn copy_path_nests_under_task() {
    assert_eq!(
        copy_relpath("7", "sub/b.txt"),
        PathBuf::from("proof/7/sub/b.txt")
    );
}

#[test]
fn refusal_at_cap_is_none_above_is_sentence() {
    let at = pf("shot.mov", 100, "h");
    assert_eq!(refusal(&at, 100), None);
    let big = pf("shot.mov", 157_286_400, "h");
    let msg = refusal(&big, 104_857_600).unwrap();
    assert!(msg.starts_with("proof file shot.mov is 157286400 bytes, above the 104857600-byte"));
    assert!(msg.contains("proof_max_bytes"));
}

fn run_script(dir: &std::path::Path) -> String {
    let argv = listing_argv();
    let out = Command::new("sh")
        .args(argv.iter().skip(1))
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn script_lists_real_files() {
    let root = std::env::temp_dir().join(format!("moor-proof-{}-real", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let proof = root.join(PROOF_DIR);
    std::fs::create_dir_all(proof.join("sub")).unwrap();
    std::fs::write(proof.join("a.png"), b"abc").unwrap();
    std::fs::write(proof.join("sub/b.txt"), b"").unwrap();
    let files = parse_listing(&run_script(&root));
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(
        files,
        vec![
            pf(
                "a.png",
                3,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            ),
            pf(
                "sub/b.txt",
                0,
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            ),
        ]
    );
}

#[test]
fn script_absent_dir_prints_nothing() {
    let root = std::env::temp_dir().join(format!("moor-proof-{}-none", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let out = run_script(&root);
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(out, "");
}
