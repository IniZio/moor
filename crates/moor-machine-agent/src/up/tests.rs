#![allow(clippy::unwrap_used, reason = "test code")]

use super::docker_env;

/// A lookup over `pairs`.
fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |name| {
        pairs
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| (*v).to_owned())
    }
}

#[test]
fn the_service_follows_the_docker_the_person_points_at() {
    let get = lookup(&[
        ("DOCKER_HOST", "unix:///run/user/1000/docker.sock"),
        ("DOCKER_CONTEXT", "rootless"),
        ("DOCKER_CONFIG", "/home/ada/.docker"),
        ("HOME", "/home/ada"),
    ]);
    assert_eq!(
        docker_env(&get),
        [
            ("DOCKER_HOST", "unix:///run/user/1000/docker.sock"),
            ("DOCKER_CONTEXT", "rootless"),
            ("DOCKER_CONFIG", "/home/ada/.docker"),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
    );
}

#[test]
fn a_docker_variable_that_is_unset_or_empty_stays_out_of_the_unit() {
    let get = lookup(&[("DOCKER_HOST", ""), ("DOCKER_CONTEXT", "rootless")]);
    assert_eq!(
        docker_env(&get),
        vec![("DOCKER_CONTEXT".to_owned(), "rootless".to_owned())]
    );
}
