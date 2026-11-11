//! The answers of the port forward requests.

/// `value` of `forward-open` and `forward-close`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ForwardView {
    /// The port on the Box's loopback.
    pub port: u16,
}

/// `value` of `forward`, sent after the connection's last byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ForwardedView {
    /// Bytes from the person's computer into the Box.
    pub bytes_in: u64,
    /// Bytes from the Box back to the person's computer.
    pub bytes_out: u64,
}
