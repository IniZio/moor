//! The host-key check every Box ssh call must keep.
//!
//! moor runs ssh to a Box with the words `crabbox ssh` printed for its lease. Those words are
//! what pins the Box's host key, so moor refuses to use them when they do not: ssh keeps the
//! first value it reads for an option, so the first `StrictHostKeyChecking` must be `yes`, and
//! no `UserKnownHostsFile` may be `/dev/null`.

use moor_core::seam::driver::DriverError;

/// Ok when `words`, the ssh arguments for a Box, pin its host key.
pub(super) fn check(words: &[String]) -> Result<(), DriverError> {
    let mut strict = None;
    let mut words = words.iter();
    while let Some(word) = words.next() {
        let option = match word.strip_prefix("-o") {
            Some("") => match words.next() {
                Some(next) => next.as_str(),
                None => break,
            },
            Some(joined) => joined,
            None => continue,
        };
        let (name, value) = option
            .split_once(|c: char| c == '=' || c.is_whitespace())
            .unwrap_or((option, ""));
        let value = value.trim_start_matches(|c: char| c == '=' || c.is_whitespace());
        if name.eq_ignore_ascii_case("StrictHostKeyChecking") {
            strict.get_or_insert(value);
        } else if name.eq_ignore_ascii_case("UserKnownHostsFile")
            && value.split_whitespace().any(|f| f == "/dev/null")
        {
            return Err(refused("it sets UserKnownHostsFile=/dev/null"));
        }
    }
    match strict {
        Some(v) if v.eq_ignore_ascii_case("yes") => Ok(()),
        Some(_) => Err(refused(
            "its first StrictHostKeyChecking is not yes, and ssh keeps the first",
        )),
        None => Err(refused("it lacks StrictHostKeyChecking=yes")),
    }
}

/// The refusal of words that do not pin the host key, saying `why`.
fn refused(why: &str) -> DriverError {
    DriverError::Failed {
        summary: format!(
            "moor refuses the ssh options crabbox printed for this Box: {why}, so the Box's host \
             key would not be checked"
        ),
    }
}

#[cfg(test)]
mod tests;
