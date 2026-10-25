use super::*;

#[test]
fn argv_passes_caller_text_verbatim_as_the_prompt() {
    let t = TempDir::new().unwrap();
    let text = "hello 'quoted' $x";
    let c = LaunchCtx {
        mcp: std::collections::BTreeMap::new(),
        prompt: LaunchPrompt::Text(text.into()),
        ..ctx(t.path())
    };
    assert_eq!(Claude.argv(&c), strings(&["--", text]));
}

#[test]
fn argv_keeps_a_dash_led_text_out_of_the_option_parser() {
    let t = TempDir::new().unwrap();
    let c = LaunchCtx {
        mcp: std::collections::BTreeMap::new(),
        model: Some("opus".into()),
        prompt: LaunchPrompt::Text("-rf hello".into()),
        ..proxied(t.path())
    };
    assert_eq!(
        Claude.argv(&c),
        strings(&[
            "--model",
            "opus",
            "--permission-mode",
            "bypassPermissions",
            "--",
            "-rf hello"
        ])
    );
}

#[test]
fn argv_has_no_prompt_when_the_launch_asks_for_none() {
    let t = TempDir::new().unwrap();
    let c = LaunchCtx {
        mcp: std::collections::BTreeMap::new(),
        prompt: LaunchPrompt::None,
        ..proxied(t.path())
    };
    assert_eq!(
        Claude.argv(&c),
        strings(&["--permission-mode", "bypassPermissions"])
    );
    let bare = LaunchCtx {
        mcp: std::collections::BTreeMap::new(),
        prompt: LaunchPrompt::None,
        ..ctx(t.path())
    };
    assert!(Claude.argv(&bare).is_empty());
}
