//! The caller's user and the secrets its Boxes use.
//!
//! The controller only says whether a secret is set, never its value.

use moor_core::request::StartNeedsReq;
use moor_core::view::user::StartNeedsValue;
use moor_core::Request;

use super::{answer, Check, Inputs, Status};

/// The user check, then one `secret` check per name a start needs.
pub(super) fn checks(i: &Inputs<'_>) -> Vec<Check> {
    if i.target.is_none() {
        let fix = "set controller.socket; your user and secrets live on the controller";
        return vec![Check::warn(
            "user",
            "not checked: no controller is set",
            fix,
        )];
    }
    let needs = match answer::<StartNeedsValue>(
        i.ask,
        Request::StartNeeds(StartNeedsReq {
            identity: None,
            repo: i.repo.clone(),
        }),
    ) {
        Ok(needs) => needs,
        Err(p) => return vec![Check::failed("user", p)],
    };
    let user = if needs.user.is_some() {
        Check::ok("user", "enrolled")
    } else {
        Check::fail("user", "not enrolled", "run `moor user enroll`")
    };
    if user.status == Status::Fail {
        return vec![user];
    }
    let secrets = needs.needs.iter().map(|n| {
        let name = n.as_str();
        if needs.missing.contains(n) {
            Check::fail("secret", format!("{name}: not set"), how_to_set(i, name))
        } else {
            Check::ok("secret", format!("{name}: set"))
        }
    });
    std::iter::once(user).chain(secrets).collect()
}

/// How to set secret `name`.
fn how_to_set(i: &Inputs<'_>, name: &str) -> String {
    let page = i
        .public_url
        .map(|u| format!(", or open {}/me/secrets", u.trim_end_matches('/')))
        .unwrap_or_default();
    format!("run `moor user secret set {name}` on the host{page}")
}
