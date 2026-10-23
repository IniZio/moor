//! Whom the front speaks for. A Slack user's Boxes belong to `slack:<team>/<user>` on the
//! controller, so every request and watch the front opens asserts that identity.

use moor_client::{Client, ClientError};
use moor_core::controller::FrameErrorKind;
use moor_core::request::Request;
use moor_core::response::Response;
use moor_core::user::Identity;

use crate::credentials::SlackUser;

/// The namespace of the controller's `slack` front socket.
const NAMESPACE: &str = "slack";

/// Longest team id accepted.
const TEAM_MAX: usize = 32;

/// A Slack workspace id, such as `T0123ABCD`, as `auth.test` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Team(String);

impl Team {
    /// The team id, when it is 1 to [`TEAM_MAX`] capital letters or digits.
    pub(crate) fn parse(id: &str) -> Result<Self, String> {
        let shaped = !id.is_empty()
            && id.len() <= TEAM_MAX
            && id
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
        if shaped {
            Ok(Self(id.to_owned()))
        } else {
            Err(format!("slack gave the team id `{id}`, which is not one"))
        }
    }
}

/// The controller socket and the team, which together make a client for each Slack user.
#[derive(Debug, Clone)]
pub(crate) struct Link {
    /// Client for the controller's `slack` front socket; it asserts no identity itself.
    client: Client,
    /// The workspace the front serves.
    team: Team,
}

impl Link {
    /// A link over `client`, for the Slack users of `team`.
    pub(crate) const fn new(client: Client, team: Team) -> Self {
        Self { client, team }
    }

    /// The identity of Slack user `user`, or why `user` is not a Slack user id.
    pub(crate) fn identity(&self, user: &str) -> Result<Identity, String> {
        let user =
            SlackUser::parse(user).ok_or_else(|| format!("`{user}` is not a Slack user id"))?;
        Identity::parse(&format!("{NAMESPACE}:{}/{}", self.team.0, user.as_str()))
    }

    /// A client that asserts `identity` on every request and watch.
    pub(crate) fn client_as(&self, identity: Identity) -> Client {
        self.client.clone().acting_as(identity)
    }

    /// A client that asserts Slack user `user` on every request and watch. A `user` that is
    /// no Slack user id gets the error frame a malformed request would, which proves nothing ran.
    pub(crate) fn acting(&self, user: &str) -> Result<Client, ClientError> {
        let identity = self.identity(user).map_err(|message| ClientError::Frame {
            kind: FrameErrorKind::Malformed,
            message,
        })?;
        Ok(self.client_as(identity))
    }

    /// Runs `request` as Slack user `user`.
    pub(crate) fn request(&self, user: &str, request: Request) -> Result<Response, ClientError> {
        self.acting(user)?.request(request, |_| {})
    }
}
