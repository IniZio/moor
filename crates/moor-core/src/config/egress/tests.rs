use super::*;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_string()).collect()
}

fn has(list: &[String], h: &str) -> bool {
    list.iter().any(|g| g == h)
}

#[test]
fn served_hosts_dropped_downloads_kept() {
    let got = effective_allow_list(&[], &[]).unwrap();
    for gone in ["api.github.com", "api.anthropic.com"] {
        assert!(!has(&got, gone), "{gone}");
    }
    for want in [
        "github.com",
        "*.githubusercontent.com",
        "mise.run",
        "nodejs.org",
    ] {
        assert!(has(&got, want), "{want}");
    }
}

#[test]
fn served_repo_entries_rejected_with_path() {
    for host in ["api.github.com", "API.Anthropic.com"] {
        let e = effective_allow_list(&[], &s(&["a.example.com", host])).unwrap_err();
        assert_eq!(e.path, "egress[1]");
        assert!(e.reason.contains("moor proxy serves it"), "{e}");
    }
}

#[test]
fn apex_and_wildcard_are_separate() {
    let got = effective_allow_list(&[], &s(&["*.example.org", "example.org"])).unwrap();
    assert!(has(&got, "*.example.org") && has(&got, "example.org"));
    let wild = effective_allow_list(&[], &s(&["*.example.org"])).unwrap();
    assert!(!has(&wild, "example.org"));
}

#[test]
fn invalid_entries_rejected() {
    for bad in ["", "localhost", "1.2.3.4", "a b.com", "*.", "ex/ample.com"] {
        let e = effective_allow_list(&[], &s(&[bad])).unwrap_err();
        assert_eq!(e.path, "egress[0]", "{bad}");
    }
}

#[test]
fn provider_capability_table() {
    let cases = [
        ("sprites", false, true),
        ("sprites", true, true),
        ("namespace-devbox", false, false),
        ("namespace-devbox", true, true),
        ("local-container", false, false),
        ("local-container", true, false),
        ("ssh", true, false),
    ];
    for (p, enforce, want) in cases {
        assert_eq!(provider_applies_policy(p, enforce), want, "{p} {enforce}");
    }
}

#[test]
fn unenforced_note_only_namespace_without_opt_in() {
    assert!(unenforced_note("namespace-devbox", false).is_some());
    assert!(unenforced_note("namespace-devbox", true).is_none());
    assert!(unenforced_note("sprites", false).is_none());
    assert!(unenforced_note("local-container", false).is_none());
}

#[test]
fn item_applied_lists_allow() {
    let it = egress_item(&[], &s(&["a.example.com"]), true).unwrap();
    assert_eq!(it.name, "allow");
    assert!(it.detail.contains("a.example.com") && it.detail.contains("github.com"));
    assert!(!it.detail.contains("api.anthropic.com"));
    let empty = egress_item(&[], &[], true).unwrap();
    assert_eq!(empty.name, "allow");
    assert!(empty.detail.contains("github.com"));
    assert_eq!(empty.hosts, effective_allow_list(&[], &[]).unwrap());
}

#[test]
fn item_unsupported_never_counts_zero() {
    let none = egress_item(&[], &[], false).unwrap();
    assert_eq!(
        (none.name, none.detail.as_str()),
        ("policy", "no egress policy")
    );
    assert!(!none.detail.contains(" 0 "));
    assert_eq!(none.hosts, effective_allow_list(&[], &[]).unwrap());
    let one = egress_item(&[], &s(&["a.example.com"]), false).unwrap();
    assert_eq!(
        one.detail,
        "no egress policy (Box provider cannot apply 1 egress entry)"
    );
    let two = egress_item(&[], &s(&["a.example.com", "b.example.com"]), false).unwrap();
    assert_eq!(
        two.detail,
        "no egress policy (Box provider cannot apply 2 egress entries)"
    );
    assert_ne!(none.detail, two.detail);
}

#[test]
fn served_domains_rejected_only_when_provider_applies_policy() {
    for host in ["api.github.com", "api.anthropic.com"] {
        assert!(egress_item(&[], &s(&[host]), true).is_err(), "{host}");
        assert!(egress_item(&[], &s(&[host]), false).is_ok(), "{host}");
    }
}

#[test]
fn effective_list_is_base_user_repo_deduplicated() {
    let got = effective_allow_list(
        &s(&["U.example.com", "github.com"]),
        &s(&["r.example.com", "u.example.com"]),
    )
    .unwrap();
    let tail: Vec<&str> = got.iter().rev().take(2).map(String::as_str).collect();
    assert_eq!(tail, ["r.example.com", "u.example.com"]);
    assert_eq!(got.iter().filter(|h| *h == "github.com").count(), 1);
    let bad = effective_allow_list(&s(&["nope"]), &[]).unwrap_err();
    assert!(bad.to_string().contains("egress.allow[0]"), "{bad}");
}

#[test]
fn item_detail_lists_exactly_the_effective_hosts_with_user_entries() {
    let user = s(&["u.example.com"]);
    for repo in [s(&[]), s(&["r.example.com"])] {
        let it = egress_item(&user, &repo, true).unwrap();
        let want = effective_allow_list(&user, &repo).unwrap();
        assert_eq!(it.name, "allow");
        assert_eq!(it.detail, want.join(", "));
        assert_eq!(it.hosts, want);
        assert!(it.detail.contains("u.example.com"));
    }
}

#[test]
fn item_unsupported_hosts_empty_when_list_invalid() {
    let it = egress_item(&[], &s(&["api.github.com"]), false).unwrap();
    assert!(it.hosts.is_empty());
}

#[test]
fn item_unsupported_counts_user_and_repo_entries() {
    let it = egress_item(&s(&["u.example.com"]), &s(&["r.example.com"]), false).unwrap();
    assert_eq!(
        it.detail,
        "no egress policy (Box provider cannot apply 2 egress entries)"
    );
}
