//! What a Box that died says of itself: its places ended, so moor did not resume it.

/// Why moor did not resume a Box whose places ended as `why` says.
#[must_use]
pub fn sentence(why: &str) -> String {
    format!("its machine stopped, so moor did not resume it: {why}")
}

#[cfg(test)]
mod tests;
