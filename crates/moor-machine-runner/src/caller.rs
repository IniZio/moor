//! Who is calling: the three headers the Box's Proxy sets on every request.

use http::HeaderMap;
use moor_core::BoxId;
use rmcp::ErrorData;

/// Header carrying the calling Box's numeric store id.
const BOX: &str = "X-Moor-Box";
/// Header carrying the attached machine's name.
const MACHINE: &str = "X-Moor-Machine";
/// Header carrying the machine's ssh target, such as `user@host`.
const SSH: &str = "X-Moor-Machine-Ssh";

/// The Box a request comes from and the machine attached to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Caller {
    /// The calling Box.
    pub(crate) box_id: BoxId,
    /// The machine's name, for messages.
    pub(crate) machine: String,
    /// The ssh target the host reaches the machine at.
    pub(crate) ssh: String,
}

impl Caller {
    /// Reads the caller from the request headers, naming the first one missing or malformed.
    pub(crate) fn from_headers(headers: &HeaderMap) -> Result<Self, ErrorData> {
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| refused(format!("missing header {name}")))
        };
        let raw = header(BOX)?;
        let box_id = raw
            .parse::<i64>()
            .ok()
            .filter(|n| *n > 0)
            .map(BoxId::new)
            .ok_or_else(|| refused(format!("header {BOX} is not a Box id: {raw}")))?;
        let machine = header(MACHINE)?;
        let ssh = header(SSH)?;
        if ssh.starts_with('-') || ssh.contains(char::is_whitespace) {
            return Err(refused(format!("header {SSH} is not an ssh target: {ssh}")));
        }
        Ok(Self {
            box_id,
            machine,
            ssh,
        })
    }

    /// The Box's copy on the machine, relative to the machine user's home.
    pub(crate) fn remote_dir(&self) -> String {
        format!("moor/{}", self.box_id.get())
    }
}

/// An MCP error for a request the runner will not serve.
fn refused(message: String) -> ErrorData {
    ErrorData::invalid_request(message, None)
}
