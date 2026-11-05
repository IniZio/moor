//! The Box's login is refreshed after the last step that can change the Box user.

use super::profile::{set_profile, world_answering, HEAD};
use super::*;

const LAST_STEP: &str = "sh setup.sh";

#[test]
fn the_login_is_refreshed_once_after_the_last_setup_step() {
    let w = world_answering(None, HEAD, None);
    set_profile(&w, None);
    run(&w.cx, &req("demo")).unwrap();
    let execs = w.driver.execs();
    let mut setup = execs.iter().enumerate();
    let (at, (handle, ..)) = setup
        .rfind(|(_, (_, spec, _))| spec.argv.join(" ").contains(LAST_STEP))
        .unwrap();
    let refreshed = w.driver.refreshed_logins();
    let on_this_box = refreshed.iter().filter(|(h, _)| h == handle);
    let after: Vec<usize> = on_this_box
        .map(|(_, execs_by_then)| *execs_by_then)
        .collect();
    let once_after_setup = matches!(after.as_slice(), [n] if *n > at);
    assert!(once_after_setup, "{refreshed:?} after exec {at}");
}
