use super::*;
use crate::config::{parse_repo, parse_user};

mod seal;

fn salt(b: u8) -> CacheSalt {
    CacheSalt::new([b; 64])
}

fn repo(owner: &str, name: &str) -> ForgeRepo {
    ForgeRepo {
        owner: owner.to_owned(),
        name: name.to_owned(),
    }
}

fn layers(repo_yaml: &str, user_yaml: &str) -> CacheLayers {
    CacheLayers {
        repo: parse_repo(".moor.yaml", repo_yaml).unwrap().cache,
        operator: parse_user("config.yaml", user_yaml).unwrap().cache,
    }
}

fn plan(l: &CacheLayers) -> CachePlan {
    CachePlan::build(UserId::new(7), Some(&repo("o", "r")), l, &salt(1)).unwrap()
}

/// `(name, paths)` of each planned volume.
fn shape(p: &CachePlan) -> Vec<(String, Vec<String>)> {
    p.volumes()
        .iter()
        .map(|v| {
            let paths = v.paths.iter().map(|p| p.as_str().to_owned()).collect();
            (v.name.as_str().to_owned(), paths)
        })
        .collect()
}

fn names(p: &CachePlan) -> Vec<String> {
    shape(p).into_iter().map(|(n, _)| n).collect()
}

fn refusal(path: &str) -> String {
    CachePath::parse(path).unwrap_err().to_string()
}

#[test]
fn paths_take_the_home_opt_and_var_cache_roots_only() {
    for ok in [
        "~/.gradle/caches",
        "/opt/android-sdk",
        "/var/cache/apt/",
        "~/a/b+c@1",
        "/var/lib/containerd",
        "/var/lib/containers/",
    ] {
        assert!(CachePath::parse(ok).is_ok(), "{ok} is a legal cache path");
    }
    for bad in [
        "~",
        "~/",
        "/opt",
        "/opt/",
        "/etc/x",
        "/var/lib/docker",
        "/var/lib/other",
        "/var/lib",
        "/var/lib/containerd/io.containerd.content.v1.content",
        "/var/lib/containers2",
        "relative/x",
        "~/a//b",
        "~/a/../b",
        "~/./a",
        "~/a b",
        "~/a,b",
        "~/$X",
    ] {
        assert!(CachePath::parse(bad).is_err(), "{bad} must be refused");
    }
    assert_eq!(
        CachePath::parse("/var/cache/apt/").unwrap().as_str(),
        "/var/cache/apt"
    );
}

#[test]
fn every_owned_path_is_refused_equal_above_and_below() {
    let owned = owned_paths();
    let missing: Vec<&str> = [
        "~/.moor-proxy",
        "~/.gradle/gradle.properties",
        "~/.gradle/moor-proxy-truststore.p12",
        "~/moor",
        "~/.ssh",
        "~/.moor",
        "~/.claude",
        "~/.config/gh",
    ]
    .into_iter()
    .filter(|w| !owned.iter().any(|o| o == w))
    .collect();
    assert!(missing.is_empty(), "not owned: {missing:?} in {owned:?}");
    for o in &owned {
        let parent = o.rsplit_once('/').unwrap().0;
        for p in [o.clone(), format!("{o}/below")] {
            assert!(refusal(&p).contains(o.as_str()), "{p} meets {o}");
        }
        if parent != "~" {
            assert!(
                refusal(parent).contains("moor writes"),
                "{parent} holds {o}"
            );
        }
    }
    assert!(refusal("~/.moor/runs").contains("~/.moor"));
    assert!(CachePath::parse("~/.gradle/caches").is_ok());
    assert!(
        CachePath::parse("~/moorish").is_ok(),
        "only whole segments overlap"
    );
}

#[test]
fn moor_home_directories_are_refused() {
    for p in ["~/.moor-cred-1-abc", "~/.moor-dotfiles", "~/.moor-cp-x/y"] {
        assert!(refusal(p).contains(".moor-"), "{p}");
    }
}

#[test]
fn names_follow_the_pattern() {
    for ok in ["a", "gradle", "android-sdk", "x1", &"a".repeat(32)] {
        assert!(CacheName::parse(ok).is_ok(), "{ok}");
    }
    for bad in ["", "1a", "-a", "Gradle", "a_b", "a.b", &"a".repeat(33)] {
        assert!(CacheName::parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn presets_expand_to_the_table() {
    let got = shape(&plan(&layers("", "")));
    let want: Vec<(&str, Vec<&str>)> = vec![
        ("npm", vec!["~/.cache/npm"]),
        ("yarn", vec!["~/.cache/yarn"]),
        ("pnpm", vec!["~/.local/share/pnpm/store"]),
        ("bun", vec!["~/.bun/install/cache"]),
        ("docker", vec!["~/docker", "/var/lib/containerd"]),
        ("podman", vec!["~/.local/share/containers"]),
        ("gradle", vec!["~/.gradle/caches", "~/.gradle/wrapper"]),
        ("dart", vec!["~/.pub-cache"]),
        (
            "rust",
            vec!["~/.cargo/registry", "~/.cargo/git", "~/.rustup"],
        ),
        ("go", vec!["~/go/pkg/mod", "~/.cache/go-build"]),
    ];
    let want: Vec<(String, Vec<String>)> = want
        .into_iter()
        .map(|(n, ps)| (n.to_owned(), ps.into_iter().map(str::to_owned).collect()))
        .collect();
    assert_eq!(got, want);
    assert_eq!(presets::names().len(), want.len(), "every preset parses");
}

#[test]
fn presets_follow_the_caches_the_exec_env_points_tools_at() {
    let (npm, yarn) = crate::exec_env::tool_caches();
    let got = shape(&plan(&layers("", "")));
    assert!(got.contains(&("npm".to_owned(), vec![npm.replace("$HOME/", "~/")])));
    assert!(got.contains(&("yarn".to_owned(), vec![yarn.replace("$HOME/", "~/")])));
}

#[test]
fn only_docker_has_a_seal() {
    let p = plan(&layers("", ""));
    let sealed: Vec<&str> = p
        .volumes()
        .iter()
        .filter(|v| !v.seal().is_empty())
        .map(|v| v.name.as_str())
        .collect();
    assert_eq!(sealed, ["docker"]);
}

#[test]
fn a_sealed_volume_gets_a_group_of_its_own() {
    let p = plan(&layers("", ""));
    let group = |name: &str| {
        let v = p
            .volumes()
            .iter()
            .find(|v| v.name.as_str() == name)
            .unwrap();
        assert_eq!(v.key, format!("{}-{name}", v.group));
        v.group.clone()
    };
    let (docker, npm) = (group("docker"), group("npm"));
    assert_eq!(docker, format!("{npm}x"));
    for other in ["yarn", "gradle", "rust", "podman"] {
        assert_eq!(group(other), npm, "{other} shares the repo group");
    }
}

#[test]
fn presets_false_or_a_drop_list_in_either_layer_removes_them() {
    assert!(plan(&layers("cache: {presets: false}", "")).is_empty());
    assert!(plan(&layers("", "cache: {presets: false}")).is_empty());
    let dropped = names(&plan(&layers(
        "cache: {presets: [docker, podman]}",
        "cache: {presets: [go]}",
    )));
    assert_eq!(
        dropped,
        ["npm", "yarn", "pnpm", "bun", "gradle", "dart", "rust"]
    );
    let err = parse_repo(".moor.yaml", "cache: {presets: [dockr]}").unwrap_err();
    assert!(err.to_string().contains("cache.presets[0]"), "{err}");
}

#[test]
fn a_box_without_a_repo_gets_only_operator_volumes() {
    let l = layers(
        "",
        "cache: {volumes: [{name: cargo-bin, path: ~/.cargo/bin}]}",
    );
    let p = CachePlan::build(UserId::new(7), None, &l, &salt(1)).unwrap();
    assert_eq!(names(&p), ["cargo-bin"]);
}

#[test]
fn a_declared_volume_displaces_the_preset_it_meets() {
    let repo_yaml =
        "cache: {volumes: [{name: wrapper, path: ~/.gradle/wrapper}, {name: npm, path: /opt/npm}]}";
    let user_yaml = "cache: {volumes: [{name: registry, path: ~/.cargo/registry}]}";
    let got = names(&plan(&layers(repo_yaml, user_yaml)));
    for gone in ["gradle", "rust"] {
        assert!(!got.contains(&gone.to_owned()), "{gone} yields: {got:?}");
    }
    assert_eq!(got.iter().filter(|n| *n == "npm").count(), 1, "{got:?}");
    assert!(got.ends_with(&["wrapper".into(), "npm".into(), "registry".into()]));
}

#[test]
fn duplicate_names_and_overlapping_paths_are_refused() {
    for (yaml, key) in [
        (
            "cache: {volumes: [{name: a, path: ~/x}, {name: a, path: ~/y}]}",
            "cache.volumes[1]",
        ),
        (
            "cache: {volumes: [{name: a, path: ~/x}, {name: b, path: ~/x/y}]}",
            "cache.volumes[1]",
        ),
        (
            "cache: {volumes: [{name: a, path: ~/x/y}, {name: b, path: ~/x}]}",
            "cache.volumes[1]",
        ),
        (
            "cache: {volumes: [{name: a, path: ~/.gradle}]}",
            "cache.volumes[0].path",
        ),
    ] {
        let err = parse_repo(".moor.yaml", yaml).unwrap_err().to_string();
        assert!(err.contains(key), "{yaml}: {err}");
        let err = parse_user("config.yaml", yaml).unwrap_err().to_string();
        assert!(err.contains(key), "operator {yaml}: {err}");
    }
    let l = layers(
        "cache: {volumes: [{name: sdk, path: /opt/sdk}]}",
        "cache: {volumes: [{name: tools, path: /opt/sdk/tools}]}",
    );
    let err = CachePlan::build(UserId::new(7), Some(&repo("o", "r")), &l, &salt(1)).unwrap_err();
    assert_eq!(err, CacheError::Overlap("sdk".into(), "tools".into()));
}

#[test]
fn a_layer_declares_at_most_eight_volumes() {
    let vols: Vec<String> = (0..9)
        .map(|i| format!("{{name: v{i}, path: ~/v{i}}}"))
        .collect();
    let yaml = format!("cache: {{volumes: [{}]}}", vols.join(", "));
    let err = parse_repo(".moor.yaml", &yaml).unwrap_err().to_string();
    assert!(err.contains("at most 8"), "{err}");
}

#[test]
fn unknown_cache_keys_are_refused() {
    for yaml in [
        "cache: {scope: user}",
        "cache: {volumes: [{name: a, path: ~/a, scope: user}]}",
        "cache: {volumes: [{name: a, path: ~/a, key: abc}]}",
    ] {
        assert!(parse_repo(".moor.yaml", yaml).is_err(), "{yaml}");
    }
}

fn key_of(user: i64, forge: &ForgeRepo, by: &CacheSalt, cfg: &CacheLayers, name: &str) -> String {
    let built = CachePlan::build(UserId::new(user), Some(forge), cfg, by).unwrap();
    let v = built
        .volumes()
        .iter()
        .find(|v| v.name.as_str() == name)
        .unwrap();
    assert_eq!(v.key, format!("{}-{name}", v.group));
    v.key.clone()
}

#[test]
fn keys_are_stable_and_scoped_to_user_repo_salt_and_origin() {
    let l = layers(
        "",
        "cache: {volumes: [{name: cargo-bin, path: ~/.cargo/bin}]}",
    );
    let r = repo("Owner", "Repo");
    let base = key_of(7, &r, &salt(1), &l, "gradle");
    assert_eq!(base, key_of(7, &r, &salt(1), &l, "gradle"), "stable");
    assert_eq!(
        base,
        key_of(7, &repo("owner", "repo"), &salt(1), &l, "gradle"),
        "case-blind"
    );
    assert_ne!(base, key_of(8, &r, &salt(1), &l, "gradle"), "per user");
    assert_ne!(
        base,
        key_of(7, &repo("owner", "other"), &salt(1), &l, "gradle"),
        "per repo"
    );
    assert_ne!(base, key_of(7, &r, &salt(2), &l, "gradle"), "per salt");
    let group = |k: String| k.split_once('-').unwrap().0.to_owned();
    let user = group(key_of(7, &r, &salt(1), &l, "cargo-bin"));
    assert_ne!(
        group(base.clone()),
        user,
        "operator volumes are a group of their own"
    );
    assert_eq!(
        user,
        group(key_of(7, &repo("x", "y"), &salt(1), &l, "cargo-bin")),
        "across repos"
    );
    assert_eq!(group(base).len(), 16);
    let charset = |k: &str| {
        k.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    assert!(charset(&user));
}

#[test]
fn a_repo_volume_never_gets_the_operator_scope() {
    let r = repo("o", "r");
    let as_repo = layers("cache: {volumes: [{name: shared, path: ~/shared}]}", "");
    let as_operator = layers("", "cache: {volumes: [{name: shared, path: ~/shared}]}");
    let repo_key = key_of(7, &r, &salt(1), &as_repo, "shared");
    let operator_key = key_of(7, &r, &salt(1), &as_operator, "shared");
    assert_ne!(repo_key, operator_key);
    assert_eq!(
        repo_key.split_once('-').unwrap().0,
        key_of(7, &r, &salt(1), &as_repo, "gradle")
            .split_once('-')
            .unwrap()
            .0,
        "a repo volume shares the presets' repo group"
    );
}

#[test]
fn salt_round_trips_through_hex_and_never_prints() {
    let s = CacheSalt::new(std::array::from_fn(|i| u8::try_from(i).unwrap()));
    assert_eq!(CacheSalt::from_hex(&s.to_hex()), Some(s.clone()));
    assert_eq!(CacheSalt::from_hex("00"), None);
    assert_eq!(CacheSalt::from_hex(&"zz".repeat(64)), None);
    assert_eq!(format!("{s:?}"), "CacheSalt(..)");
}

fn docker_stays_sealed(p: &CachePlan) {
    let docker = p
        .volumes()
        .iter()
        .find(|v| v.name.as_str() == "docker")
        .expect("the docker preset is kept");
    assert!(docker.group.ends_with('x'), "{}", docker.group);
    assert!(!docker.seal().is_empty());
    let paths: Vec<&str> = docker.paths.iter().map(CachePath::as_str).collect();
    assert_eq!(paths, ["~/docker", "/var/lib/containerd"]);
}

#[test]
fn a_declared_volume_on_a_sealed_preset_is_dropped_and_docker_stays_sealed() {
    let own = |path: &str| format!("cache: {{volumes: [{{name: mine, path: {path}}}]}}");
    let cases = [
        (own("~/docker"), String::new(), Origin::Repo),
        (own("/var/lib/containerd"), String::new(), Origin::Repo),
        (own("~/docker/volumes"), String::new(), Origin::Repo),
        (String::new(), own("~/docker"), Origin::Operator),
    ];
    for (repo_yaml, user_yaml, origin) in cases {
        let p = plan(&layers(&repo_yaml, &user_yaml));
        docker_stays_sealed(&p);
        assert!(
            !names(&p).contains(&"mine".to_owned()),
            "{repo_yaml}{user_yaml}"
        );
        assert!(names(&p).contains(&"npm".to_owned()));
        let want = Dropped {
            origin,
            volume: CacheName::parse("mine").unwrap(),
            preset: CacheName::parse("docker").unwrap(),
        };
        assert_eq!(p.dropped(), [want], "{repo_yaml}{user_yaml}");
    }
}

#[test]
fn dropping_one_volume_keeps_the_others_and_ignores_it_in_clash_checks() {
    let p = plan(&layers(
        "cache: {volumes: [{name: mine, path: ~/docker}, {name: tools, path: /opt/tools}]}",
        "cache: {volumes: [{name: more, path: ~/docker/x}]}",
    ));
    docker_stays_sealed(&p);
    assert!(names(&p).ends_with(&["tools".into()]), "{:?}", names(&p));
    let gone: Vec<&str> = p.dropped().iter().map(|d| d.volume.as_str()).collect();
    assert_eq!(gone, ["mine", "more"]);
}

#[test]
fn an_unsealed_preset_still_yields_to_a_declared_volume() {
    let p = plan(&layers(
        "cache: {volumes: [{name: mine, path: ~/.cache/npm}]}",
        "",
    ));
    let got = names(&p);
    assert!(got.contains(&"mine".to_owned()), "{got:?}");
    assert!(!got.contains(&"npm".to_owned()), "{got:?}");
    assert!(p.dropped().is_empty());
}

fn dropped_mine(p: &CachePlan, origin: Origin) {
    assert!(!names(p).contains(&"docker".to_owned()), "{:?}", names(p));
    assert!(!names(p).contains(&"mine".to_owned()), "{:?}", names(p));
    let want = Dropped {
        origin,
        volume: CacheName::parse("mine").unwrap(),
        preset: CacheName::parse("docker").unwrap(),
    };
    assert_eq!(p.dropped(), [want]);
}

#[test]
fn docker_paths_stay_reserved_with_presets_off() {
    let p = plan(&layers(
        "cache: {presets: false, volumes: [{name: mine, path: ~/docker}]}",
        "",
    ));
    dropped_mine(&p, Origin::Repo);
}

#[test]
fn docker_paths_stay_reserved_when_a_drop_list_names_docker() {
    let p = plan(&layers(
        "cache: {presets: [docker]}",
        "cache: {volumes: [{name: mine, path: /var/lib/containerd}]}",
    ));
    dropped_mine(&p, Origin::Operator);
    assert!(names(&p).contains(&"npm".to_owned()));
}

#[test]
fn a_dropped_volume_says_who_declared_it_and_which_preset_won() {
    let p = plan(&layers(
        "",
        "cache: {volumes: [{name: mine, path: ~/docker}]}",
    ));
    assert_eq!(
        p.dropped().first().unwrap().to_string(),
        "operator cache volume mine meets the sealed docker preset, which keeps its directory, \
         so it is dropped"
    );
}

#[test]
fn the_box_scope_is_the_repo_groups_hash_and_only_a_repo_box_with_volumes_has_one() {
    let p = plan(&layers("", ""));
    let npm = p.volumes().first().unwrap();
    assert_eq!(p.scope(), Some(npm.group.as_str()));
    assert_eq!(p.scope().map(str::len), Some(16));

    let none = plan(&layers("cache: {presets: false}", ""));
    assert!(none.is_empty());
    assert_eq!(none.scope(), None);

    let l = layers(
        "",
        "cache: {volumes: [{name: cargo-bin, path: ~/.cargo/bin}]}",
    );
    let norepo = CachePlan::build(UserId::new(7), None, &l, &salt(1)).unwrap();
    assert_eq!(norepo.scope(), None);
    assert_eq!(CachePlan::none().scope(), None);
}
