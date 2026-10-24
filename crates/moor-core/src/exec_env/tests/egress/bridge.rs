//! The egress config follows `docker0` as the fake `ip` reports it.

use super::{running, Apt, FakeBox};

fn bridge_lines(b: &FakeBox) -> Vec<String> {
    let conf = b.conf();
    conf.lines()
        .skip_while(|l| !l.starts_with("PidFile "))
        .skip(1)
        .take_while(|l| !l.starts_with("Deny ") && *l != "Allow 172.16.0.0/12")
        .map(str::to_owned)
        .collect()
}

fn set_docker0(b: &FakeBox, route: Option<&str>) {
    let file = b.home.join("docker0.route");
    match route {
        Some(r) => std::fs::write(file, r).unwrap(),
        None => std::fs::remove_file(file).unwrap(),
    }
}

fn rerun(b: &FakeBox) {
    b.port.close();
    let (ok, out, err) = b.run();
    assert!(ok, "{out}{err}");
}

fn pid(b: &FakeBox) -> String {
    std::fs::read_to_string(b.home.join(".moor/egress.pid"))
        .unwrap()
        .trim()
        .to_owned()
}

#[test]
fn without_docker0_tinyproxy_listens_only_on_loopback() {
    let b = FakeBox::new(0, Apt::Missing, true);
    let (ok, out, err) = b.run();
    assert!(ok, "{out}{err}");
    let conf = b.conf();
    let listens: Vec<&str> = conf.lines().filter(|l| l.starts_with("Listen ")).collect();
    assert_eq!(listens, ["Listen 127.0.0.1"], "{conf}");
    assert!(!conf.contains("0.0.0.0"), "{conf}");
    assert_eq!(bridge_lines(&b), Vec::<String>::new());
}

#[test]
fn with_docker0_tinyproxy_also_listens_on_its_address_and_answers_its_network() {
    let b = FakeBox::new(0, Apt::Missing, true);
    set_docker0(
        &b,
        Some("10.200.0.0/24 proto kernel scope link src 10.200.0.1\n"),
    );
    let (ok, out, err) = b.run();
    assert!(ok, "{out}{err}");
    assert!(b.conf().contains("\nListen 127.0.0.1\n"), "{}", b.conf());
    assert_eq!(
        bridge_lines(&b),
        ["Listen 10.200.0.1", "Allow 10.200.0.0/24"]
    );
}

#[test]
fn docker0_that_appears_moves_or_goes_restarts_tinyproxy_once_each() {
    let b = FakeBox::new(0, Apt::Missing, true);
    assert!(b.run().0);
    let steps: [(Option<&str>, &[&str]); 3] = [
        (
            Some("172.17.0.0/16 src 172.17.0.1 linkdown\n"),
            &["Listen 172.17.0.1", "Allow 172.17.0.0/16"],
        ),
        (
            Some("172.30.0.0/20 src 172.30.0.1\n"),
            &["Listen 172.30.0.1", "Allow 172.30.0.0/20"],
        ),
        (None, &[]),
    ];
    for (n, (route, want)) in steps.into_iter().enumerate() {
        let old = pid(&b);
        set_docker0(&b, route);
        rerun(&b);
        assert_eq!(bridge_lines(&b), want, "{route:?}");
        assert_eq!(b.starts(), n + 2, "{route:?}: {}", b.log());
        assert!(
            !running(&old),
            "{route:?}: the old tinyproxy was not stopped"
        );
    }
    let (ok, out, err) = b.run();
    assert!(ok, "{out}{err}");
    assert_eq!(b.starts(), 4, "an unchanged bridge restarted tinyproxy");
}

fn set_subnets(b: &FakeBox, routes: &str) {
    std::fs::write(b.home.join("kernel.routes"), routes).unwrap();
}

/// The lines after `PidFile` that decide a source's fate, in order.
fn access_lines(b: &FakeBox) -> Vec<String> {
    b.conf()
        .lines()
        .skip_while(|l| !l.starts_with("PidFile "))
        .skip(1)
        .filter(|l| l.starts_with("Allow ") || l.starts_with("Deny "))
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_subnet_of_the_box_inside_the_docker_pool_is_denied_before_the_pool_is_allowed() {
    let b = FakeBox::new(0, Apt::Missing, true);
    set_docker0(&b, Some("172.17.0.0/16 src 172.17.0.1 linkdown\n"));
    set_subnets(
        &b,
        "172.17.0.0/16 dev docker0 proto kernel scope link src 172.17.0.1 linkdown\n\
         172.18.0.0/16 dev br-0123abcd proto kernel scope link src 172.18.0.1\n\
         172.19.5.0/24 dev veth12 proto kernel scope link src 172.19.5.1\n\
         172.31.0.0/16 dev eth0 proto kernel scope link src 172.31.5.4\n\
         10.8.0.1 dev tun0 proto kernel scope link src 10.8.0.2\n",
    );
    let (ok, out, err) = b.run();
    assert!(ok, "{out}{err}");
    assert_eq!(
        access_lines(&b),
        [
            "Allow 172.17.0.0/16",
            "Deny 172.31.0.0/16",
            "Deny 10.8.0.1",
            "Allow 172.16.0.0/12"
        ],
        "{}",
        b.conf()
    );
    let conf = b.conf();
    let at = |needle: &str| conf.find(needle).unwrap();
    assert!(at("Allow 127.0.0.1") < at("Deny 172.31.0.0/16"), "{conf}");
}

#[test]
fn a_box_without_connected_subnets_has_no_deny_lines() {
    let b = FakeBox::new(0, Apt::Missing, true);
    let (ok, out, err) = b.run();
    assert!(ok, "{out}{err}");
    assert_eq!(access_lines(&b), ["Allow 172.16.0.0/12"], "{}", b.conf());
}

#[test]
fn a_change_in_the_subnets_of_the_box_restarts_tinyproxy_once() {
    let b = FakeBox::new(0, Apt::Missing, true);
    set_subnets(
        &b,
        "172.31.0.0/16 dev eth0 proto kernel scope link src 172.31.5.4\n",
    );
    assert!(b.run().0);
    assert_eq!(b.starts(), 1);
    let old = pid(&b);
    set_subnets(
        &b,
        "10.0.0.0/24 dev eth0 proto kernel scope link src 10.0.0.4\n",
    );
    rerun(&b);
    assert_eq!(
        access_lines(&b),
        ["Deny 10.0.0.0/24", "Allow 172.16.0.0/12"]
    );
    assert_eq!(b.starts(), 2, "{}", b.log());
    assert!(!running(&old));
    let (ok, out, err) = b.run();
    assert!(ok, "{out}{err}");
    assert_eq!(b.starts(), 2, "unchanged subnets restarted tinyproxy");
}
