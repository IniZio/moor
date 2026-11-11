//! Arguments of `forward`.

use clap::Args;
use moor_client::port_forward::Spec;
use moor_core::BoxName;

use super::box_name;

/// Arguments of `forward`.
#[derive(Debug, Args)]
pub(super) struct ForwardArgs {
    /// Box name.
    #[arg(value_parser = box_name)]
    name: BoxName,
    /// The port in the Box.
    port: u16,
    /// The port on this computer; the Box's port when left out.
    #[arg(long)]
    local_port: Option<u16>,
}

impl From<ForwardArgs> for Spec {
    fn from(a: ForwardArgs) -> Self {
        Self {
            box_name: a.name.as_str().to_owned(),
            port: a.port,
            local_port: a.local_port,
        }
    }
}
