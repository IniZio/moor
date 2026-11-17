//! A Box's override is kept with the Box, replaced whole, and removed with `None`.

use moor_core::domain::decision::{Decision, NewBox, SpecFields};
use moor_core::BoxName;

use crate::store::tests::open;

#[test]
fn an_override_is_stored_replaced_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let new = |name: &str, config: Option<&str>| {
        Decision::insert(NewBox {
            name: BoxName::parse_allow_bare(name).unwrap(),
            repo: None,
            spec: SpecFields {
                config: config.map(str::to_owned),
                ..SpecFields::default()
            },
        })
    };
    let with = store
        .apply(new("a", Some("env: {A: box}\n")))
        .unwrap()
        .box_id;
    let without = store.apply(new("b", None)).unwrap().box_id;
    assert_eq!(
        store.box_config_override(with).unwrap().as_deref(),
        Some("env: {A: box}\n")
    );
    assert_eq!(store.box_config_override(without).unwrap(), None);

    store
        .set_box_config_override(with, Some("setup: [x]\n"))
        .unwrap();
    assert_eq!(
        store.box_config_override(with).unwrap().as_deref(),
        Some("setup: [x]\n")
    );
    assert_eq!(store.box_config_override(without).unwrap(), None);

    store.set_box_config_override(with, None).unwrap();
    assert_eq!(store.box_config_override(with).unwrap(), None);
}
