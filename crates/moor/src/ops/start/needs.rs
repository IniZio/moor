//! The secrets a start needs from its requester's user: the ones its Proxy rules inject.

use std::collections::BTreeSet;

use moor_core::request::StartNeedsReq;
use moor_core::seam::forge::ForgeRepo;
use moor_core::user::{SecretName, UserId};
use moor_core::view::user::StartNeedsValue;
use moor_core::ErrorKind;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::proxy::plan;

/// `start-needs`: what a start of `req.repo` would need from the caller's user, starting nothing.
///
/// # Errors
/// Usage outside a controller socket; Failed when the rules do not resolve.
pub(crate) fn run(cx: &Cx, req: &StartNeedsReq) -> Result<StartNeedsValue, CommandError> {
    let (who, user) = cx.requester()?;
    let needs = needs(cx, req.repo.as_ref())?;
    let have: BTreeSet<SecretName> = match user {
        Some(user) => cx.store()?.user_secret_names(user)?.into_iter().collect(),
        None => BTreeSet::new(),
    };
    let missing = needs
        .iter()
        .filter(|n| !have.contains(*n))
        .cloned()
        .collect();
    Ok(StartNeedsValue {
        identity: who.clone(),
        user,
        needs,
        missing,
    })
}

/// The caller's user when it holds every secret a start of `repo` needs. Otherwise a refusal
/// whose value is the [`StartNeedsValue`], naming every missing secret at once.
///
/// # Errors
/// As [`run`], and Refused when the user is missing or lacks a secret.
pub(super) fn require(cx: &Cx, repo: Option<&ForgeRepo>) -> Result<UserId, CommandError> {
    let req = StartNeedsReq {
        identity: None,
        repo: repo.cloned(),
    };
    let value = run(cx, &req)?;
    if let (Some(user), true) = (value.user, value.missing.is_empty()) {
        return Ok(user);
    }
    let sets = value
        .missing
        .iter()
        .map(|n| format!("moor user secret set {}", n.as_str()));
    let steps: Vec<String> = value
        .user
        .is_none()
        .then(|| "moor user enroll".to_owned())
        .into_iter()
        .chain(sets)
        .collect();
    let shown =
        serde_json::to_value(&value).map_err(|e| CommandError::new(ErrorKind::Internal, e))?;
    Err(CommandError::new(
        ErrorKind::Refused,
        format!("log in first: {}", steps.join(", then ")),
    )
    .with_value(shown))
}

/// Every secret the Proxy rules inject for a Box of `repo`, sorted.
fn needs(cx: &Cx, repo: Option<&ForgeRepo>) -> Result<Vec<SecretName>, CommandError> {
    let rules = plan(cx, repo)
        .map_err(|e| CommandError::new(ErrorKind::Failed, e))?
        .rules;
    let names: BTreeSet<SecretName> = rules
        .iter()
        .map(|r| SecretName::parse(&r.credential))
        .collect::<Result<_, _>>()
        .map_err(|e| CommandError::new(ErrorKind::Failed, e))?;
    Ok(names.into_iter().collect())
}
