use super::UserMachine;

fn labels(l: &[&str]) -> Vec<String> {
    l.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn a_good_machine_has_its_labels_sorted_and_deduplicated() {
    let m = UserMachine::parse(
        "mac-1",
        "dev@build.example.com:2222",
        &labels(&["ios-sim", "macos", "ios-sim"]),
    )
    .unwrap();
    assert_eq!(m.labels, ["ios-sim", "macos"]);
    assert_eq!(m.ssh, "dev@build.example.com:2222");
}

#[test]
fn these_targets_are_accepted() {
    for target in [
        "dev@host",
        "dev@10.0.0.7",
        "dev@10.0.0.7:22",
        "dev@a.b-c.example:65535",
        "dev@[::1]",
        "dev@[fe80::1]:2200",
        "_svc.user-1@h",
    ] {
        assert!(UserMachine::parse("m", target, &[]).is_ok(), "{target}");
    }
}

#[test]
fn these_targets_are_refused() {
    for target in [
        "-oProxyCommand=x",
        "-o@host",
        "dev@-oProxyCommand=x",
        "a b@c",
        "dev@a b",
        "root@host;rm",
        "root@host$(x)",
        "root@host`x`",
        "root@host|x",
        "root@host&x",
        "root@ho\nst",
        "host",
        "@host",
        "dev@",
        "dev@host:",
        "dev@host:0",
        "dev@host:65536",
        "dev@host:22x",
        "dev@host:22:22",
        "dev@::1",
        "dev@[::1",
        "dev@[::1]x",
        "dev@[nope]",
        "dev@1.2.3",
        "dev@999.1.1.1",
        "dev@.host",
        "dev@host.",
        "dev@a..b",
        "dev@-host",
        "dev@host-",
        "dev@h_st",
        "dev@host/x",
        "dev@ho~st",
        "d/ev@host",
        "dev@a@b",
    ] {
        assert!(UserMachine::parse("m", target, &[]).is_err(), "{target:?}");
    }
}

#[test]
fn these_names_are_refused() {
    let long = "a".repeat(65);
    for name in [
        "", "Mac", "1mac", "-mac", "_mac", "mac book", "mac.1", "mac/1", &long,
    ] {
        assert!(UserMachine::parse(name, "u@h", &[]).is_err(), "{name:?}");
    }
    assert!(UserMachine::parse(&"a".repeat(64), "u@h", &[]).is_ok());
}

#[test]
fn these_labels_are_refused() {
    let long = "a".repeat(33);
    for label in ["", "Mac", "-ios", "ios_sim", "ios sim", "ios.sim", &long] {
        assert!(
            UserMachine::parse("m", "u@h", &labels(&[label])).is_err(),
            "{label:?}"
        );
    }
    assert!(UserMachine::parse("m", "u@h", &labels(&["9p", &"a".repeat(32)])).is_ok());
}

#[test]
fn at_most_sixteen_distinct_labels() {
    let many: Vec<String> = (0..17).map(|i| format!("l{i}")).collect();
    assert!(UserMachine::parse("m", "u@h", many.get(..16).unwrap()).is_ok());
    assert!(UserMachine::parse("m", "u@h", &many).is_err());
    let repeated: Vec<String> = (0..20).map(|_| "same".to_owned()).collect();
    assert!(UserMachine::parse("m", "u@h", &repeated).is_ok());
}
