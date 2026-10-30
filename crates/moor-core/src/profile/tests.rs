use super::*;

fn err(text: &str) -> String {
    parse(text).unwrap_err().to_string()
}

#[test]
fn full_manifest_parses() {
    let m = parse(
        "claude:\n  model: opus\n  marketplaces:\n    tools: https://github.com/acme/agent-tools\n    extra: https://github.com/acme/extra.git\n  plugins: [lint@tools, fmt@extra]\ncodex:\n  model: gpt-5.5\n",
    )
    .unwrap();
    assert_eq!(m.claude.model.as_deref(), Some("opus"));
    assert_eq!(m.codex.model.as_deref(), Some("gpt-5.5"));
    assert_eq!(
        m.claude.marketplaces.get("extra"),
        Some(&GithubRepo(ForgeRepo::new("acme", "extra")))
    );
    assert_eq!(
        m.claude.plugins,
        vec![
            PluginRef {
                name: "lint".into(),
                marketplace: "tools".into()
            },
            PluginRef {
                name: "fmt".into(),
                marketplace: "extra".into()
            },
        ]
    );
    assert_eq!(
        repos(&m),
        vec![
            ForgeRepo::new("acme", "extra"),
            ForgeRepo::new("acme", "agent-tools")
        ]
    );
}

#[test]
fn empty_file_is_an_empty_manifest() {
    for text in ["", "\n", "{}", "# nothing\n"] {
        assert_eq!(parse(text).unwrap(), Manifest::default(), "{text:?}");
    }
    assert!(repos(&Manifest::default()).is_empty());
}

#[test]
fn repos_lists_a_shared_repo_once() {
    let m = parse(
        "claude:\n  marketplaces:\n    a: https://github.com/acme/tools\n    b: https://github.com/acme/tools.git\n",
    )
    .unwrap();
    assert_eq!(repos(&m), vec![ForgeRepo::new("acme", "tools")]);
}

#[test]
fn marketplace_must_be_a_github_https_repo() {
    for url in [
        "http://github.com/acme/tools",
        "https://gitlab.com/acme/tools",
        "git@github.com:acme/tools.git",
        "https://github.com/acme",
        "https://github.com/acme/tools/tree/main",
        "acme/tools",
    ] {
        let e = err(&format!("claude:\n  marketplaces:\n    t: {url}\n"));
        assert!(e.contains("claude.marketplaces.t"), "{url}: {e}");
    }
}

#[test]
fn plugin_needs_a_marketplace() {
    for p in ["lint", "lint@", "@tools", "a@b@c"] {
        let e = err(&format!(
            "claude:\n  marketplaces:\n    tools: https://github.com/acme/t\n  plugins: [\"{p}\"]\n"
        ));
        assert!(e.contains("claude.plugins[0]"), "{p}: {e}");
    }
}

#[test]
fn plugin_must_name_a_declared_marketplace() {
    let e = err(
        "claude:\n  marketplaces:\n    tools: https://github.com/acme/t\n  plugins: [lint@other]\n",
    );
    assert!(
        e.contains("claude.plugins[0]") && e.contains("`other`"),
        "{e}"
    );
    assert!(err("claude:\n  plugins: [lint@tools]\n").contains("not declared"));
}

#[test]
fn unknown_keys_are_refused() {
    assert!(err("gemini: {}\n").contains("gemini"));
    assert!(err("claude:\n  effort: high\n").contains("effort"));
    assert!(err("codex:\n  effort: high\n").contains("effort"));
}

#[test]
fn model_order_is_flag_then_profile_then_config() {
    let cases = [
        (Some("f"), Some("p"), Some("c"), Some("f")),
        (None, Some("p"), Some("c"), Some("p")),
        (None, None, Some("c"), Some("c")),
        (Some("f"), None, Some("c"), Some("f")),
        (None, None, None, None),
    ];
    for (flag, profile, config, want) in cases {
        assert_eq!(model(flag, profile, config).as_deref(), want);
    }
}

#[test]
fn plugin_sources_are_the_github_repos_of_the_named_plugins() {
    let json = r#"{"name":"tools","owner":{"name":"a"},"plugins":[
      {"name":"lint","source":{"source":"github","repo":"acme/lint"}},
      {"name":"fmt","source":{"source":"url","url":"https://github.com/acme/fmt.git"}},
      {"name":"local","source":"./plugins/local"},
      {"name":"other","source":{"source":"url","url":"https://example.com/x.git"}},
      {"name":"unwanted","source":{"source":"github","repo":"acme/unwanted"}}]}"#;
    let got = plugin_sources(json, "tools", &["lint", "fmt", "local", "other"]).unwrap();
    assert_eq!(
        got,
        [
            ForgeRepo::new("acme", "lint"),
            ForgeRepo::new("acme", "fmt")
        ]
    );
    assert!(plugin_sources("not json", "tools", &["lint"]).is_err());
    assert_eq!(
        plugin_sources(json, "kit", &["lint"]).unwrap_err(),
        "marketplace kit is named tools"
    );
}

#[test]
fn a_git_subdir_source_reads_its_repo() {
    let json = r#"{"name":"tools","plugins":[
      {"name":"sub","source":{"source":"git-subdir","url":"https://github.com/acme/mono.git","path":"plugins/sub"}}]}"#;
    assert_eq!(
        plugin_sources(json, "tools", &["sub"]).unwrap(),
        [ForgeRepo::new("acme", "mono")]
    );
}

#[test]
fn profile_marketplace_needs_no_declaration_and_adds_no_proxy_repo() {
    let m =
        parse("claude:\n  plugins: [lint@profile]\ncodex:\n  plugins: [fmt@profile]\n").unwrap();
    assert_eq!(m.claude.plugins[0].marketplace, PROFILE_MARKETPLACE);
    assert!(repos(&m).is_empty());
}

#[test]
fn profile_cannot_be_declared_as_a_marketplace() {
    let e = err("codex:\n  marketplaces:\n    profile: https://github.com/acme/tools\n");
    assert!(e.contains("codex.marketplaces.profile"), "{e}");
}

#[test]
fn codex_plugins_follow_the_same_rules_as_claude() {
    let m = parse(
        "codex:\n  marketplaces:\n    tools: https://github.com/acme/agent-tools\n  plugins: [lint@tools]\n",
    )
    .unwrap();
    assert_eq!(repos(&m), vec![ForgeRepo::new("acme", "agent-tools")]);
    let e = err("codex:\n  plugins: [lint@nowhere]\n");
    assert!(
        e.contains("codex.plugins[0]") && e.contains("nowhere"),
        "{e}"
    );
    let e = err("codex:\n  marketplaces:\n    t: https://example.com/a/b\n");
    assert!(e.contains("codex.marketplaces"), "{e}");
}
