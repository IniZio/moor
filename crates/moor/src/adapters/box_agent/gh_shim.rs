//! The `gh` stand-in installed on the Box PATH; it reaches GitHub only through the moor Proxy.

/// The POSIX sh source of the shim, installed as `gh`.
const GH_SHIM: &str = include_str!("gh_shim.sh");

/// The script bytes and the file mode to install them with.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the launch path installs it later")
)]
pub(crate) const fn gh_shim() -> (&'static [u8], u32) {
    (GH_SHIM.as_bytes(), 0o755)
}

#[cfg(test)]
mod tests;
