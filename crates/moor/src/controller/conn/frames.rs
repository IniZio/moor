//! Frames the connection sends on its own account: hello answers and stray-frame errors.

use moor_core::controller::{ClientFrame, FrameErrorKind, ServerFrame, PROTOCOL};

/// The error for a bytes or resize frame `id` sent outside a running `attach`.
pub(super) fn stray(id: String) -> ServerFrame {
    ServerFrame::Error {
        id: Some(id),
        kind: FrameErrorKind::Malformed,
        message: "bytes and resize frames belong to a running attach".to_owned(),
    }
}

/// The controller's answer to a client hello of `protocol`.
pub(super) fn hello_reply(protocol: u32) -> ServerFrame {
    if protocol == PROTOCOL {
        return ServerFrame::Hello { protocol: PROTOCOL };
    }
    ServerFrame::Error {
        id: None,
        kind: FrameErrorKind::UnsupportedProtocol,
        message: format!("client speaks protocol {protocol}, controller speaks {PROTOCOL}"),
    }
}

/// The error for `frame`, a watch or a request, sent before the hello.
pub(super) fn hello_required(frame: ClientFrame) -> ServerFrame {
    let (id, message) = match frame {
        ClientFrame::Request { id, .. } => (
            Some(id.clone()),
            format!("request {id} arrived before hello"),
        ),
        ClientFrame::Watch { .. } => (None, "watch arrived before hello".to_owned()),
        _ => (None, "a frame arrived before hello".to_owned()),
    };
    ServerFrame::Error {
        id,
        kind: FrameErrorKind::HelloRequired,
        message,
    }
}
