//! The Boxes and Sessions a request names, which the controller lets a caller name only when
//! it may act on them.

use super::{BoxTarget, Request};
use crate::name::{BoxName, SessionId};

/// One Box or Session a request names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Named {
    /// A Box by id, or by name the newest Box of that name.
    Box(BoxTarget),
    /// The open Box of this name, when one exists: `start` attaches to it, and otherwise makes
    /// a new Box.
    Attach(BoxName),
    /// A Session, which names its Box when it has one.
    Session(SessionId),
}

impl Request {
    /// Every Box and Session this request names. Requests that list Boxes name none; their
    /// handlers show only what the caller may see.
    #[must_use]
    pub fn names(&self) -> Vec<Named> {
        let named = |name: &BoxName| Named::Box(name.clone().into());
        // A name that does not parse names no Box; the handler refuses it the same way.
        let bare = |name: &str| BoxName::parse_allow_bare(name).ok().map(|n| named(&n));
        let target = |target: &BoxTarget| vec![Named::Box(target.clone())];
        let session = |id: SessionId| vec![Named::Session(id)];
        match self {
            Self::Start(r) => [
                Some(Named::Attach(r.name.clone())),
                r.parent.map(Named::Session),
            ]
            .into_iter()
            .flatten()
            .collect(),
            Self::OpenSession(r) => [bare(&r.box_name), r.parent.map(Named::Session)]
                .into_iter()
                .flatten()
                .collect(),
            Self::EndSession(r) => bare(&r.box_name).into_iter().collect(),
            Self::OpenHostSession(r) => r.parent.map(Named::Session).into_iter().collect(),
            Self::Sessions(r) => r.parent.map(Named::Session).into_iter().collect(),
            Self::Show(r) => r.target.iter().map(|t| Named::Box(t.clone())).collect(),
            Self::Run(r) => vec![named(&r.box_name)],
            Self::Wait(r) => vec![named(&r.box_name)],
            Self::Cp(r) => vec![named(&r.box_name)],
            Self::Open(r) | Self::Sleep(r) | Self::Wake(r) | Self::Connect(r) => {
                vec![named(&r.box_name)]
            }
            Self::WaitEvents(r) => r.boxes.iter().map(named).collect(),
            Self::Steer(r) => target(&r.target),
            Self::Close(r) => target(&r.target),
            Self::Artifacts(r) => target(&r.target),
            Self::ArtifactRead(r) => target(&r.target),
            Self::ShowSession(r) => session(r.session),
            Self::UpdateHostSession(r) => session(r.session),
            Self::EndHostSession(r) => session(r.session),
            Self::BindSession(r) => session(r.session),
            Self::As(r) => r.request.names(),
            Self::StartNeeds(_)
            | Self::SessionBinding(_)
            | Self::Gate(_)
            | Self::Env(_)
            | Self::Repos(_)
            | Self::Whoami(_)
            | Self::Enroll(_)
            | Self::SecretSet(_)
            | Self::SecretDelete(_)
            | Self::SecretNames(_)
            | Self::LinkCode(_)
            | Self::LinkRedeem(_)
            | Self::Version {} => Vec::new(),
        }
    }
}
