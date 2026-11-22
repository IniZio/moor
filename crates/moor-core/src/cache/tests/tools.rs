use super::*;

fn tools(p: &CachePlan) -> &PlannedVolume {
    p.volumes()
        .iter()
        .find(|v| v.name.as_str() == "tools")
        .expect("the tools preset is planned")
}

#[test]
fn what_the_profile_installs_is_a_preset() {
    let p = plan(&layers("", ""));
    let paths: Vec<&str> = tools(&p).paths.iter().map(CachePath::as_str).collect();
    assert_eq!(paths, ["~/.bun/bin", "~/.local/bin", "~/.claude/plugins"]);
}

#[test]
fn the_tools_volume_is_keyed_to_the_user_and_repo_like_every_preset() {
    let l = layers("", "");
    let at = |user, r: &ForgeRepo| {
        let p = CachePlan::build(UserId::new(user), Some(r), &l, &salt(1)).unwrap();
        (tools(&p).key.clone(), p.scope().unwrap().to_owned())
    };
    let (key, scope) = at(7, &repo("o", "r"));
    assert_eq!(key, format!("{scope}-tools"));
    assert_eq!(at(7, &repo("o", "r")).0, key);
    assert_eq!(at(7, &repo("O", "R")).0, key);
    assert_ne!(at(8, &repo("o", "r")).0, key);
    assert_ne!(at(7, &repo("o", "other")).0, key);
}

#[test]
fn only_the_plugins_directory_of_the_agent_state_may_be_cached() {
    assert!(CachePath::parse("~/.claude/plugins").is_ok());
    for refused in [
        "~/.claude",
        "~/.claude/plugins/x",
        "~/.claude/projects",
        "~/.ssh",
    ] {
        assert!(CachePath::parse(refused).is_err(), "{refused}");
    }
}

#[test]
fn pnpm_is_told_its_store_is_the_volume() {
    let store = "~/.local/share/pnpm/store".to_owned();
    let env = layers("", "").env(Some(&repo("o", "r")));
    assert_eq!(
        env,
        [
            ("npm_config_store_dir".to_owned(), store.clone()),
            ("pnpm_config_store_dir".to_owned(), store),
        ]
    );
}

#[test]
fn no_kept_pnpm_preset_means_no_pnpm_env() {
    let r = repo("o", "r");
    assert!(layers("cache: {presets: [pnpm]}", "")
        .env(Some(&r))
        .is_empty());
    assert!(layers("cache: {presets: false}", "")
        .env(Some(&r))
        .is_empty());
    assert!(layers("", "").env(None).is_empty());
}
