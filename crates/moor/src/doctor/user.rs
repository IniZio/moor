//! The caller's user and the secrets its Boxes use.
//!
//! The vault is sealed with a key only the controller holds, so these checks see secret names
//! and never values.

use moor_core::request::{AsReq, StartNeedsReq};
use moor_core::user::SecretName;
use moor_core::view::user::{SecretNamesValue, StartNeedsValue};
use moor_core::Request;

use super::{answer, Check, Inputs};

/// Secret that holds the GitHub token.
const GITHUB: &str = "github";
/// Secret that holds the Claude token.
const CLAUDE: &str = "claude";

/// The user, github and claude checks.
pub(super) fn checks(i: &Inputs<'_>) -> Vec<Check> {
    if i.target.is_none() {
        let fix = "set controller.socket; your user and secrets live on the controller";
        return ["user", GITHUB, CLAUDE]
            .into_iter()
            .map(|name| Check::warn(name, "not checked: no controller is set", fix))
            .collect();
    }
    let needs = match answer::<StartNeedsValue>(
        i.ask,
        Request::StartNeeds(StartNeedsReq {
            identity: None,
            repo: None,
        }),
    ) {
        Ok(needs) => needs,
        Err(p) => return vec![Check::failed("user", p)],
    };
    let user = enrolled(i, &needs);
    if user.status == super::Status::Fail && needs.user.is_none() {
        return vec![user];
    }
    let names = answer::<SecretNamesValue>(i.ask, Request::SecretNames(AsReq { identity: None }))
        .map(|v| v.names);
    vec![user, github(i, &names), claude(i, &names)]
}

/// How to set secret `name`.
fn how_to_set(i: &Inputs<'_>, name: &str) -> String {
    let page = i
        .public_url
        .map(|u| format!(", or open {}/me/secrets", u.trim_end_matches('/')))
        .unwrap_or_default();
    format!("run `moor user secret set {name}` on the host{page}")
}

/// Whether the caller has a user and every secret a start needs.
fn enrolled(i: &Inputs<'_>, needs: &StartNeedsValue) -> Check {
    if needs.user.is_none() {
        return Check::fail("user", "not enrolled", "run `moor user enroll`");
    }
    if needs.missing.is_empty() {
        let list = names(&needs.needs);
        let all = if list.is_empty() {
            "needs no secret".to_owned()
        } else {
            format!("has {list}")
        };
        return Check::ok("user", format!("enrolled; {all}"));
    }
    let first = needs.missing.first().map_or("NAME", SecretName::as_str);
    Check::fail(
        "user",
        format!("missing secrets: {}", names(&needs.missing)),
        how_to_set(i, first),
    )
}

/// The names, comma separated.
fn names(list: &[SecretName]) -> String {
    list.iter()
        .map(SecretName::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The stored github secret, by name.
fn github(i: &Inputs<'_>, stored: &Result<Vec<SecretName>, super::Problem>) -> Check {
    stored_secret(i, stored, GITHUB, "the CLI cannot call GitHub with it")
}

/// The stored claude secret, by name.
fn claude(i: &Inputs<'_>, stored: &Result<Vec<SecretName>, super::Problem>) -> Check {
    stored_secret(
        i,
        stored,
        CLAUDE,
        "its `claude setup-token` shape is not checked",
    )
}

/// Whether `name` is stored; its value is sealed, so a stored one is a warning, not a pass.
fn stored_secret(
    i: &Inputs<'_>,
    stored: &Result<Vec<SecretName>, super::Problem>,
    name: &'static str,
    unchecked: &str,
) -> Check {
    match stored {
        Err(p) => Check::warn(
            name,
            format!("secret list unavailable: {}", p.detail),
            p.fix.clone(),
        ),
        Ok(list) if list.iter().any(|s| s.as_str() == name) => Check::warn(
            name,
            format!("stored, but only the controller can open it, so {unchecked}"),
            "start a Box to prove it",
        ),
        Ok(_) => Check::fail(name, "no such secret stored", how_to_set(i, name)),
    }
}
