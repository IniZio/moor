use super::pin::{check, known_hosts_files, refusal, KnownHosts};
use super::split_words;

/// Files as a test sets them: devices and read-only paths by name.
struct Files {
    devices: &'static [&'static str],
    read_only: &'static [&'static str],
}

impl KnownHosts for Files {
    fn is_device(&self, file: &str) -> bool {
        self.devices.contains(&file)
    }
    fn can_add_to(&self, file: &str) -> bool {
        !self.read_only.contains(&file)
    }
}

const PLAIN: Files = Files {
    devices: &["/dev/null"],
    read_only: &["/ro/known_hosts"],
};

fn options(strict: &str, files: &str) -> String {
    format!("user vm\nstricthostkeychecking {strict}\nuserknownhostsfile {files}\nport 22\n")
}

#[test]
fn yes_and_accept_new_with_a_real_file_pin_the_key() {
    for strict in ["yes", "true", "accept-new"] {
        let out = options(strict, "/home/u/.ssh/known_hosts /home/u/.ssh/known_hosts2");
        assert_eq!(check(&out, &PLAIN), Ok(()), "{strict}");
    }
}

#[test]
fn checking_turned_off_is_refused() {
    for strict in ["no", "off", "false"] {
        let out = options(strict, "/home/u/.ssh/known_hosts");
        assert_eq!(
            check(&out, &PLAIN).unwrap_err(),
            "they turn host key checking off"
        );
    }
    assert!(check("userknownhostsfile /k\n", &PLAIN).is_err());
}

#[test]
fn a_device_none_or_missing_file_is_refused() {
    for files in ["/home/u/k /dev/null", "none", ""] {
        let err = check(&options("yes", files), &PLAIN).unwrap_err();
        assert!(err.contains("/dev/null or no file"), "{files}: {err}");
    }
    assert!(check("stricthostkeychecking yes\n", &PLAIN).is_err());
}

#[test]
fn accept_new_needs_a_writable_first_file() {
    let err = check(&options("accept-new", "/ro/known_hosts /w"), &PLAIN).unwrap_err();
    assert!(
        err.contains("cannot record a new key in /ro/known_hosts"),
        "{err}"
    );
    assert_eq!(check(&options("yes", "/ro/known_hosts"), &PLAIN), Ok(()));
}

#[test]
fn the_files_are_listed_in_order() {
    assert_eq!(known_hosts_files(&options("yes", "/a /b")), ["/a", "/b"]);
    assert!(known_hosts_files("port 22\n").is_empty());
}

#[test]
fn the_refusal_says_why() {
    let text = refusal("they turn host key checking off");
    assert!(text.contains("they turn host key checking off, so the Box's host key"));
}

#[test]
fn words_split_on_quotes_and_escapes() {
    assert_eq!(
        split_words("ssh '-p' '22' vm\\ x 'a b'").unwrap(),
        ["ssh", "-p", "22", "vm x", "a b"]
    );
    assert_eq!(split_words("ssh 'open"), None);
}
