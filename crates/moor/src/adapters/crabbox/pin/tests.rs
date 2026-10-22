use moor_core::seam::driver::DriverError;

use super::check;

fn words(line: &str) -> Vec<String> {
    line.split(' ').map(str::to_owned).collect()
}

fn refused(line: &str) -> String {
    match check(&words(line)) {
        Err(DriverError::Failed { summary }) => summary,
        other => unreachable!("{line}: expected a refusal, got {other:?}"),
    }
}

#[test]
fn crabbox_pinned_words_pass_in_every_spelling() {
    for line in [
        "-o StrictHostKeyChecking=yes -o UserKnownHostsFile=/lease/known_hosts u@box",
        "-oStrictHostKeyChecking=yes u@box",
        "-o stricthostkeychecking=YES u@box",
        "-i /k/id -o StrictHostKeyChecking=yes -o StrictHostKeyChecking=no u@box",
    ] {
        assert_eq!(check(&words(line)), Ok(()), "{line}");
    }
    let spaced = ["-o", "StrictHostKeyChecking yes", "u@box"].map(str::to_owned);
    assert_eq!(check(&spaced), Ok(()));
}

#[test]
fn words_without_the_pin_are_refused() {
    for line in ["u@box", "-p 22 u@box", "-o"] {
        assert!(
            refused(line).contains("lacks StrictHostKeyChecking=yes"),
            "{line}"
        );
    }
}

#[test]
fn a_weaker_host_key_option_before_the_pin_is_refused() {
    for line in [
        "-o StrictHostKeyChecking=accept-new -o StrictHostKeyChecking=yes u@box",
        "-oStrictHostKeyChecking=no -o StrictHostKeyChecking=yes u@box",
        "-o StrictHostKeyChecking=off u@box",
        "-o StrictHostKeyChecking u@box",
    ] {
        assert!(
            refused(line).contains("first StrictHostKeyChecking"),
            "{line}"
        );
    }
}

#[test]
fn a_known_hosts_file_of_dev_null_is_refused() {
    for line in [
        "-o StrictHostKeyChecking=yes -o UserKnownHostsFile=/dev/null u@box",
        "-o UserKnownHostsFile=/lease/known_hosts -o StrictHostKeyChecking=yes -oUserKnownHostsFile=/dev/null u@box",
    ] {
        assert!(refused(line).contains("/dev/null"), "{line}");
    }
    let listed = [
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "UserKnownHostsFile /lease/known_hosts /dev/null",
        "u@box",
    ]
    .map(str::to_owned);
    assert!(matches!(check(&listed), Err(DriverError::Failed { .. })));
}
