use super::*;
use std::process::Command;

fn pf(name: &str, size: u64, sha: &str) -> ProofFile {
    ProofFile {
        name: name.to_owned(),
        size,
        sha256: sha.to_owned(),
    }
}

fn hx(c: char) -> String {
    c.to_string().repeat(64)
}

#[test]
fn parse_keeps_good_lines_sorted() {
    let out = format!("b.txt\t3\t{}\nsub/a.png\t10\t{}\n", hx('b'), hx('a'));
    assert_eq!(
        parse_listing(&out),
        vec![pf("b.txt", 3, &hx('b')), pf("sub/a.png", 10, &hx('a'))]
    );
}

#[test]
fn parse_skips_malformed_lines() {
    let h = hx('a');
    let out = format!("\nnoTabs\nx\tnotnum\t{h}\nx\t1\n\t1\t{h}\nok\t1\t{h}\nx\t1\t{h}\textra\n");
    assert_eq!(parse_listing(&out), vec![pf("ok", 1, &h)]);
}

#[test]
fn parse_skips_parent_and_absolute_names() {
    let h = hx('a');
    let out =
        format!("../x\t1\t{h}\na/../b\t1\t{h}\n/etc/passwd\t1\t{h}\na//b\t1\t{h}\ngood\t1\t{h}\n");
    assert_eq!(parse_listing(&out), vec![pf("good", 1, &h)]);
}

#[test]
fn parse_requires_64_lowercase_hex_hash() {
    let good = hx('a');
    let upper = hx('A');
    let short = "a".repeat(63);
    let long = "a".repeat(65);
    let nonhex = format!("{}g", "a".repeat(63));
    let out =
        format!("u\t1\t{upper}\ns\t1\t{short}\nl\t1\t{long}\nn\t1\t{nonhex}\nok\t1\t{good}\n");
    assert_eq!(parse_listing(&out), vec![pf("ok", 1, &good)]);
}

#[test]
fn parse_split_line_yields_no_bogus_entry() {
    // A name with an embedded newline splits the record across two lines.
    let h = hx('a');
    let out = format!("bad\n\t1\t{h}\nbad\t1\n{h}\ngood\t2\t{h}\n");
    assert_eq!(parse_listing(&out), vec![pf("good", 2, &h)]);
}

#[test]
fn parse_dedupes_by_name_keeping_last() {
    let out = format!("a\t1\t{}\nb\t2\t{}\na\t3\t{}\n", hx('1'), hx('2'), hx('3'));
    assert_eq!(
        parse_listing(&out),
        vec![pf("a", 3, &hx('3')), pf("b", 2, &hx('2'))]
    );
}

#[test]
fn fresh_never_returns_a_name_twice() {
    let listing = [pf("a", 1, "h1"), pf("a", 2, "h2"), pf("b", 1, "h3")];
    let got = fresh(&listing, &[]);
    assert_eq!(got, vec![pf("a", 2, "h2"), pf("b", 1, "h3")]);
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

const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("moor-proof-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn script_does_not_follow_symlinked_moor() {
    let root = scratch("lnk-moor");
    std::fs::create_dir_all(root.join("outside/proof")).unwrap();
    std::fs::write(root.join("outside/proof/key"), b"abc").unwrap();
    std::os::unix::fs::symlink("outside", root.join(".moor")).unwrap();
    let out = run_script(&root);
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(out, "");
}

#[test]
fn script_does_not_follow_symlinked_proof() {
    let root = scratch("lnk-proof");
    std::fs::create_dir_all(root.join(".moor")).unwrap();
    std::fs::create_dir_all(root.join("outside")).unwrap();
    std::fs::write(root.join("outside/key"), b"abc").unwrap();
    std::os::unix::fs::symlink("../outside", root.join(".moor/proof")).unwrap();
    let out = run_script(&root);
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(out, "");
}

#[test]
fn script_skips_symlinked_files_and_dirs_inside() {
    let root = scratch("lnk-inside");
    let proof = root.join(PROOF_DIR);
    std::fs::create_dir_all(&proof).unwrap();
    std::fs::create_dir_all(root.join("outside")).unwrap();
    std::fs::write(root.join("outside/key"), b"abc").unwrap();
    std::fs::write(proof.join("real"), b"abc").unwrap();
    std::os::unix::fs::symlink(root.join("outside/key"), proof.join("filelink")).unwrap();
    std::os::unix::fs::symlink(root.join("outside"), proof.join("dirlink")).unwrap();
    let files = parse_listing(&run_script(&root));
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(files, vec![pf("real", 3, ABC)]);
}

#[test]
fn script_hashes_names_with_spaces_and_backslashes() {
    let root = scratch("odd-names");
    let proof = root.join(PROOF_DIR);
    std::fs::create_dir_all(&proof).unwrap();
    std::fs::write(proof.join("a b.png"), b"abc").unwrap();
    std::fs::write(proof.join("back\\slash.txt"), b"abc").unwrap();
    let files = parse_listing(&run_script(&root));
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(
        files,
        vec![pf("a b.png", 3, ABC), pf("back\\slash.txt", 3, ABC)]
    );
}

#[test]
fn script_drops_names_containing_a_newline() {
    let root = scratch("newline");
    let proof = root.join(PROOF_DIR);
    std::fs::create_dir_all(&proof).unwrap();
    std::fs::write(proof.join("bad\nname"), b"abc").unwrap();
    std::fs::write(proof.join("good"), b"abc").unwrap();
    let raw = run_script(&root);
    let files = parse_listing(&raw);
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(raw.lines().count(), 1, "raw: {raw:?}");
    assert_eq!(files, vec![pf("good", 3, ABC)]);
}

#[test]
fn script_falls_back_to_shasum_without_sha256sum() {
    use std::os::unix::fs::PermissionsExt;
    let root = scratch("shasum");
    let proof = root.join(PROOF_DIR);
    std::fs::create_dir_all(&proof).unwrap();
    std::fs::write(proof.join("a"), b"abc").unwrap();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for tool in ["sh", "find", "wc", "tr", "cut", "head", "awk", "touch"] {
        if let Ok(p) = which(tool) {
            std::os::unix::fs::symlink(p, bin.join(tool)).unwrap();
        }
    }
    let marker = root.join("marker");
    let shim = bin.join("shasum");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\ntouch '{}'\nexec /usr/bin/sha256sum\n",
            marker.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let argv = listing_argv();
    let out = Command::new(bin.join("sh"))
        .args(argv.iter().skip(1))
        .current_dir(&root)
        .env("PATH", &bin)
        .output()
        .unwrap();
    let took_fallback = marker.exists();
    let files = parse_listing(&String::from_utf8(out.stdout).unwrap());
    std::fs::remove_dir_all(&root).unwrap();
    assert!(took_fallback, "shasum shim was not used");
    assert_eq!(files, vec![pf("a", 3, ABC)]);
}

fn which(tool: &str) -> Result<PathBuf, ()> {
    ["/usr/bin", "/bin"]
        .iter()
        .map(|d| PathBuf::from(d).join(tool))
        .find(|p| p.exists())
        .ok_or(())
}
