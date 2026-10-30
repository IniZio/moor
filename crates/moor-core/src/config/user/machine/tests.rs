use super::UserMachine;

fn labels(l: &[&str]) -> Vec<String> {
    l.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn a_good_machine_has_its_labels_sorted_and_deduplicated() {
    let m = UserMachine::parse("mac-1", &labels(&["ios-sim", "macos", "ios-sim"])).unwrap();
    assert_eq!(m.labels, ["ios-sim", "macos"]);
}

#[test]
fn these_names_are_refused() {
    let long = "a".repeat(65);
    for name in [
        "", "Mac", "1mac", "-mac", "_mac", "mac book", "mac.1", "mac/1", &long,
    ] {
        assert!(UserMachine::parse(name, &[]).is_err(), "{name:?}");
    }
    assert!(UserMachine::parse(&"a".repeat(64), &[]).is_ok());
}

#[test]
fn these_labels_are_refused() {
    let long = "a".repeat(33);
    for label in ["", "Mac", "-ios", "ios_sim", "ios sim", "ios.sim", &long] {
        assert!(
            UserMachine::parse("m", &labels(&[label])).is_err(),
            "{label:?}"
        );
    }
    assert!(UserMachine::parse("m", &labels(&["9p", &"a".repeat(32)])).is_ok());
}

#[test]
fn at_most_sixteen_distinct_labels() {
    let many: Vec<String> = (0..17).map(|i| format!("l{i}")).collect();
    assert!(UserMachine::parse("m", many.get(..16).unwrap()).is_ok());
    assert!(UserMachine::parse("m", &many).is_err());
    let repeated: Vec<String> = (0..20).map(|_| "same".to_owned()).collect();
    assert!(UserMachine::parse("m", &repeated).is_ok());
}

#[test]
fn a_pool_provider_matches_crabbox_provider_names() {
    let m = UserMachine::parse("hv", &[]).unwrap();
    for ok in ["external", "a", "aws-spot2", &"a".repeat(64)] {
        assert_eq!(m.clone().with_pool(ok).unwrap().pool.as_deref(), Some(ok));
    }
    for bad in ["", "Ext", "9a", "a_b", "-a", "a b", "a@b", &"a".repeat(65)] {
        let err = m.clone().with_pool(bad).unwrap_err();
        assert!(err.contains("--pool"), "{bad:?}: {err}");
    }
    assert_eq!(m.pool, None);
}
