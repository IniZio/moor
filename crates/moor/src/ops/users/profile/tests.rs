use moor_core::name::ProfileSource;
use moor_core::request::UserCmd;
use moor_core::ErrorKind;
use serde_json::json;

use crate::ops::users::run;
use crate::ops::users::tests::{fixture, id};

fn set(url: &str) -> UserCmd {
    UserCmd::ProfileSet {
        source: ProfileSource::parse(url).unwrap(),
    }
}

#[test]
fn set_show_and_rm_round_trip() {
    let f = fixture();
    let me = id("slack:T1:UA");
    f.cx.store().unwrap().enroll(&me, None).unwrap();
    let none = json!({"source": null});
    assert_eq!(
        run(&f.cx, &f.vault, &me, &UserCmd::ProfileShow).unwrap(),
        none
    );

    let url = "https://github.com/alice/agent#v2";
    let set_value = run(
        &f.cx,
        &f.vault,
        &me,
        &set("https://github.com/alice/agent.git#v2"),
    )
    .unwrap();
    assert_eq!(set_value, json!({"source": url}));
    assert_eq!(
        run(&f.cx, &f.vault, &me, &UserCmd::ProfileShow).unwrap(),
        json!({"source": url})
    );

    assert_eq!(
        run(&f.cx, &f.vault, &me, &UserCmd::ProfileRemove).unwrap(),
        none
    );
    assert_eq!(
        run(&f.cx, &f.vault, &me, &UserCmd::ProfileShow).unwrap(),
        none
    );
}

#[test]
fn set_replaces_and_drops_an_earlier_ref() {
    let f = fixture();
    let me = id("slack:T1:UA");
    f.cx.store().unwrap().enroll(&me, None).unwrap();
    run(
        &f.cx,
        &f.vault,
        &me,
        &set("https://github.com/alice/agent#v2"),
    )
    .unwrap();
    run(&f.cx, &f.vault, &me, &set("https://github.com/alice/other")).unwrap();
    assert_eq!(
        run(&f.cx, &f.vault, &me, &UserCmd::ProfileShow).unwrap(),
        json!({"source": "https://github.com/alice/other"})
    );
}

#[test]
fn another_users_profile_is_invisible_and_untouched() {
    let f = fixture();
    let (a, b) = (id("slack:T1:UA"), id("slack:T1:UB"));
    let store = f.cx.store().unwrap();
    store.enroll(&a, None).unwrap();
    store.enroll(&b, None).unwrap();
    run(&f.cx, &f.vault, &a, &set("https://github.com/alice/agent")).unwrap();
    assert_eq!(
        run(&f.cx, &f.vault, &b, &UserCmd::ProfileShow).unwrap(),
        json!({"source": null})
    );
    run(&f.cx, &f.vault, &b, &UserCmd::ProfileRemove).unwrap();
    assert_eq!(
        run(&f.cx, &f.vault, &a, &UserCmd::ProfileShow).unwrap(),
        json!({"source": "https://github.com/alice/agent"})
    );
}

#[test]
fn a_profile_needs_an_enrolled_user() {
    let f = fixture();
    let err = run(&f.cx, &f.vault, &id("slack:T1:UZ"), &UserCmd::ProfileShow).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
}
