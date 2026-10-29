//! How a Driver error reads to people.

use std::fmt;

use super::tunnel::fmt_tunnel;
use super::DriverError;

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "box not found"),
            Self::Capacity { limit, holder } => fmt_capacity(f, *limit, holder.as_deref()),
            Self::Quota(m) => write!(f, "provider quota reached: {m}"),
            Self::Auth(m) => write!(f, "provider authentication failed: {m}"),
            Self::Timeout(d) => write!(f, "box operation timed out after {}s", d.as_secs()),
            Self::Unavailable(m) => write!(f, "box provider unavailable: {m}"),
            Self::Unsupported(what) => write!(f, "not supported by this provider: {what}"),
            Self::UnknownProvider(p) => write!(f, "unknown box provider `{p}`"),
            Self::Tunnel {
                failure,
                stderr_tail,
            } => fmt_tunnel(f, *failure, stderr_tail),
            Self::Transport { stderr_tail } if stderr_tail.is_empty() => {
                f.write_str("moor lost the ssh connection to the Box")
            }
            Self::Transport { stderr_tail } => {
                write!(f, "moor lost the ssh connection to the Box: {stderr_tail}")
            }
            Self::Failed { summary } => f.write_str(summary),
        }
    }
}

/// Writes the capacity message, naming limit and holder when known.
fn fmt_capacity(
    f: &mut fmt::Formatter<'_>,
    limit: Option<u32>,
    holder: Option<&str>,
) -> fmt::Result {
    write!(f, "box capacity reached")?;
    if let Some(n) = limit {
        write!(f, " (limit {n})")?;
    }
    if let Some(h) = holder {
        write!(f, ", held by {h}")?;
    }
    Ok(())
}
