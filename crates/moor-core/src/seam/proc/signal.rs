//! Signalling a process group through the [`Proc`](super::Proc) seam.
//!
//! `Proc::signal_group` returns `Ok` once the signal was delivered to the
//! group. It returns `Err` when the group no longer exists or when the pgid
//! was refused by [`check_group`].

use super::ProcError;

/// A signal that can be sent to a process group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// SIGTERM.
    Term,
    /// SIGKILL.
    Kill,
}

/// Refuses pgids that would signal more than the intended group: 0 is the
/// caller's own group and 1 is init. Adapters and fakes call this before
/// signalling.
///
/// # Errors
/// [`ProcError::Spawn`] with `InvalidInput` when `pgid <= 1`.
pub fn check_group(pgid: u32) -> Result<(), ProcError> {
    if pgid <= 1 {
        return Err(ProcError::Spawn(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("refusing to signal process group {pgid}"),
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::check_group;

    #[test]
    fn refuses_zero_and_one() {
        assert!(check_group(0).is_err());
        assert!(check_group(1).is_err());
    }

    #[test]
    fn accepts_ordinary_pgid() {
        assert!(check_group(2).is_ok());
    }
}
