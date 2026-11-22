//! The git mirror: a preset volume kept per user and repo, holding one bare mirror per forge repo.

use super::*;

/// The `git` volume's key and the mirror path of a Box of `user` on `r`.
fn mirror(user: i64, r: &ForgeRepo, l: &CacheLayers) -> (Option<String>, Option<String>) {
    let p = CachePlan::build(UserId::new(user), Some(r), l, &salt(1)).unwrap();
    let key = p
        .volumes()
        .iter()
        .find(|v| v.name.as_str() == "git")
        .map(|v| v.key.clone());
    (key, p.git_mirror(r))
}

#[test]
fn the_mirror_is_a_preset_keyed_to_the_user_and_the_forge_repo() {
    let l = layers("", "");
    let (key, path) = mirror(7, &repo("o", "r"), &l);
    let key = key.expect("the git preset is planned");
    assert_eq!(
        path.as_deref(),
        Some(r#""$HOME"/'.local/share/git-mirror/o/r.git'"#)
    );
    assert_eq!(mirror(7, &repo("O", "R"), &l), (Some(key.clone()), path));
    for (user, other) in [
        (8, repo("o", "r")),
        (7, repo("o", "other")),
        (7, repo("x", "r")),
    ] {
        assert_ne!(
            mirror(user, &other, &l).0,
            Some(key.clone()),
            "{user} {other:?}"
        );
    }
}

#[test]
fn a_box_whose_layers_drop_the_git_preset_has_no_mirror() {
    for (repo_yaml, user_yaml) in [
        ("cache: {presets: [git]}", ""),
        ("", "cache: {presets: false}"),
    ] {
        let got = mirror(7, &repo("o", "r"), &layers(repo_yaml, user_yaml));
        assert_eq!(got, (None, None), "{repo_yaml} {user_yaml}");
    }
}

#[test]
fn a_volume_declared_at_the_mirror_path_still_holds_the_mirror() {
    let l = layers(
        "",
        "cache: {volumes: [{name: mirrors, path: ~/.local/share/git-mirror}]}",
    );
    let (key, path) = mirror(7, &repo("o", "r"), &l);
    assert_eq!(key, None, "the preset yields to the declared volume");
    assert_eq!(
        path.as_deref(),
        Some(r#""$HOME"/'.local/share/git-mirror/o/r.git'"#)
    );
}
