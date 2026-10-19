use super::*;

fn af(name: &str, size: u64, sha: &str) -> ArtifactFile {
    ArtifactFile {
        name: name.to_owned(),
        size,
        sha256: sha.to_owned(),
    }
}

fn hx(c: char) -> String {
    c.to_string().repeat(64)
}

#[test]
fn name_rule() {
    for ok in ["a", "A1", "shot.png", "a_b-c.d", "0x", "a..b"] {
        assert!(valid_name(ok), "{ok}");
    }
    for bad in [
        "", ".hidden", ".", "..", "a/b", "/a", "-a", "_a", "a b", "é.png", "aé", "a\n",
    ] {
        assert!(!valid_name(bad), "{bad:?}");
    }
}

#[test]
fn sha_rule() {
    assert!(valid_sha256(&hx('a')));
    assert!(!valid_sha256(&hx('A')));
    assert!(!valid_sha256(&"a".repeat(63)));
    assert!(!valid_sha256(&format!("{}g", "a".repeat(63))));
}

#[test]
fn parse_good_lines_sorted() {
    let out = format!("b.txt\t3\t{}\n\na.png\t10\t{}\n", hx('b'), hx('a'));
    assert_eq!(
        parse_index(&out),
        (vec![af("a.png", 10, &hx('a')), af("b.txt", 3, &hx('b'))], 0)
    );
}

#[test]
fn parse_counts_malformed() {
    let h = hx('a');
    let out = format!(
        "noTabs\nx\tnotnum\t{h}\nx\t1\n\t1\t{h}\nx\t1\t{h}\textra\n../x\t1\t{h}\na/b\t1\t{h}\n.d\t1\t{h}\nu\t1\t{}\nok\t1\t{h}\n\n",
        hx('A')
    );
    assert_eq!(parse_index(&out), (vec![af("ok", 1, &h)], 9));
}

#[test]
fn parse_duplicate_last_wins() {
    let out = format!("a\t1\t{}\na\t2\t{}\n", hx('a'), hx('b'));
    assert_eq!(parse_index(&out), (vec![af("a", 2, &hx('b'))], 0));
}

#[test]
fn fresh_new_changed_unchanged() {
    let index = vec![
        af("same", 1, &hx('a')),
        af("changed", 1, &hx('b')),
        af("new", 1, &hx('c')),
    ];
    let latest = vec![af("same", 1, &hx('a')), af("changed", 1, &hx('d'))];
    assert_eq!(
        fresh(&index, &latest),
        vec![af("changed", 1, &hx('b')), af("new", 1, &hx('c'))]
    );
    assert_eq!(fresh(&index, &index), Vec::new());
    assert_eq!(fresh(&index, &[]), index);
}

#[test]
fn kind_per_extension() {
    let cases = [
        ("a.gif", ArtifactKind::Gif),
        ("a.mp4", ArtifactKind::Video),
        ("a.mov", ArtifactKind::Video),
        ("a.webm", ArtifactKind::Video),
        ("a.png", ArtifactKind::Screenshot),
        ("a.jpg", ArtifactKind::Screenshot),
        ("a.jpeg", ArtifactKind::Screenshot),
        ("a.webp", ArtifactKind::Screenshot),
        ("a.log", ArtifactKind::Log),
        ("a.txt", ArtifactKind::Log),
        ("a.PNG", ArtifactKind::Screenshot),
        ("a.GiF", ArtifactKind::Gif),
        ("a.bin", ArtifactKind::File),
        ("noext", ArtifactKind::File),
        ("a.png.bin", ArtifactKind::File),
    ];
    for (name, kind) in cases {
        assert_eq!(kind_of(name), kind, "{name}");
    }
}

#[test]
fn kind_labels_round_trip() {
    for k in [
        ArtifactKind::Screenshot,
        ArtifactKind::Gif,
        ArtifactKind::Video,
        ArtifactKind::Log,
        ArtifactKind::File,
    ] {
        assert_eq!(ArtifactKind::parse(k.as_str()), Some(k));
    }
    assert_eq!(ArtifactKind::parse("Gif"), None);
    assert_eq!(ArtifactKind::parse(""), None);
}

#[test]
fn content_type_per_extension() {
    let cases = [
        ("a.png", "image/png"),
        ("a.jpg", "image/jpeg"),
        ("a.JPEG", "image/jpeg"),
        ("a.gif", "image/gif"),
        ("a.webp", "image/webp"),
        ("a.mp4", "video/mp4"),
        ("a.mov", "video/quicktime"),
        ("a.webm", "video/webm"),
        ("a.log", "text/plain; charset=utf-8"),
        ("a.TXT", "text/plain; charset=utf-8"),
        ("a.md", "text/markdown; charset=utf-8"),
        ("a.json", "application/json"),
        ("a.pdf", "application/pdf"),
        ("a.xyz", "application/octet-stream"),
        ("noext", "application/octet-stream"),
    ];
    for (name, ct) in cases {
        assert_eq!(content_type(name), ct, "{name}");
    }
}
