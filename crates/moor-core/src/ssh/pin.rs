//! The host-key check ssh words from crabbox must pass before anything connects with them.
//!
//! ssh itself says what the words come to: `ssh -G` prints the options it would connect with,
//! after reading every flag and config file, and opens no connection. `StrictHostKeyChecking`
//! must be yes or accept-new, which records a new key and rejects a changed one. It must not be
//! no, off or false. No known-hosts file may be `/dev/null`, another device, `none` or empty,
//! and accept-new needs a first file it can write.

/// What the check asks about a known-hosts file, on the machine where ssh will read it.
pub trait KnownHosts {
    /// Whether `file` is a device, such as `/dev/null`, that can hold no host key.
    fn is_device(&self, file: &str) -> bool;
    /// Whether ssh can add a host key to `file`: it is writable, or missing in a writable dir.
    fn can_add_to(&self, file: &str) -> bool;
}

/// The known-hosts files `options`, the output of `ssh -G`, names, in ssh's order.
#[must_use]
pub fn known_hosts_files(options: &str) -> Vec<&str> {
    options
        .lines()
        .find_map(|line| line.strip_prefix("userknownhostsfile "))
        .map(|files| files.split(' ').collect())
        .unwrap_or_default()
}

/// Ok when `options`, the output of `ssh -G`, pins the host key.
///
/// # Errors
/// Why not, for [`refusal`], when checking is off or no file can keep the key.
pub fn check(options: &str, files: &impl KnownHosts) -> Result<(), String> {
    let mut strict = None;
    let mut listed = None;
    for line in options.lines() {
        match line.split_once(' ') {
            Some(("stricthostkeychecking", v)) => strict = Some(v),
            Some(("userknownhostsfile", v)) => listed = Some(v),
            _ => {}
        }
    }
    if !matches!(strict, Some("true" | "yes" | "accept-new")) {
        return Err("they turn host key checking off".to_owned());
    }
    let holds_no_keys = |f: &str| f.is_empty() || f == "none" || files.is_device(f);
    let listed = match listed {
        Some(listed) if !listed.split(' ').any(holds_no_keys) => listed,
        _ => return Err("they name /dev/null or no file for UserKnownHostsFile".to_owned()),
    };
    let first = listed.split(' ').next().unwrap_or_default();
    if strict == Some("accept-new") && !files.can_add_to(first) {
        return Err(format!(
            "accept-new cannot record a new key in {first}, which is not writable"
        ));
    }
    Ok(())
}

/// The refusal of words that do not pin the host key, saying `why`.
#[must_use]
pub fn refusal(why: &str) -> String {
    format!(
        "moor refuses the ssh options crabbox printed for this Box: {why}, so the Box's host key \
         would not be checked"
    )
}
