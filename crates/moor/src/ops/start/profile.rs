//! The requester's agent profile, resolved through the forge before the Box exists.
//!
//! No profile step ever fails the start: each failure becomes a `profile_failed` event and a
//! warning, and the Box starts without what the step would have given it.

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::ProfileStep;
use moor_core::profile::{self, GithubRepo, MANIFEST_FILE, MARKETPLACE_FILE};
use moor_core::seam::forge::ForgeRepo;
use moor_core::user::UserId;
use moor_core::{BoxId, Secret};

use super::repo::github_token;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::profile_fetch::failed;
use crate::store::box_profile::BoxProfile;
use crate::store::Store;

/// The manifest key that names the profile repo itself as a marketplace.
const PROFILE_MARKETPLACE: &str = "profile";

/// What the forge said about the requester's profile.
pub(crate) struct Gathered {
    /// The profile at its resolved commit; `None` when the ref did not resolve.
    resolved: Option<BoxProfile>,
    /// Each step that failed, with why.
    failures: Vec<(ProfileStep, String)>,
}

/// Resolves `user`'s profile, if one is set; changes nothing.
///
/// # Errors
/// [`CommandError`] when the store cannot be read.
pub(crate) fn gather(cx: &Cx, user: UserId) -> Result<Option<Gathered>, CommandError> {
    let Some(source) = cx.store()?.user_profile(user)? else {
        return Ok(None);
    };
    let failed = |detail: String| Gathered {
        resolved: None,
        failures: vec![(ProfileStep::Resolve, detail)],
    };
    let token = match github_token(cx, user) {
        Ok(token) => token,
        Err(e) => return Ok(Some(failed(e.error.to_string()))),
    };
    let at = source.git_ref.as_deref();
    let (sha, manifest) = match cx
        .forge()
        .read_file_at(&source.repo, at, MANIFEST_FILE, &token)
    {
        Ok(found) => found,
        Err(e) => return Ok(Some(failed(format!("{}: {e}", source.repo)))),
    };
    let mut failures = Vec::new();
    let mut reads = vec![source.repo.clone()];
    if let Some(text) = &manifest {
        match profile::parse(text) {
            Ok(m) => failures.extend(plugin_repos(cx, &m, &token, &mut reads)),
            Err(e) => failures.push((ProfileStep::Manifest, e.to_string())),
        }
    }
    let resolved = BoxProfile {
        repo: source.repo,
        sha,
        manifest,
        reads,
    };
    Ok(Some(Gathered {
        resolved: Some(resolved),
        failures,
    }))
}

/// Adds each marketplace repo of `m` and the GitHub source repos of the plugins `m` names to
/// `reads`, returning the marketplaces that could not be read.
fn plugin_repos(
    cx: &Cx,
    m: &profile::Manifest,
    token: &Secret,
    reads: &mut Vec<ForgeRepo>,
) -> Vec<(ProfileStep, String)> {
    let add = |repo: ForgeRepo, reads: &mut Vec<ForgeRepo>| {
        if !reads.contains(&repo) {
            reads.push(repo);
        }
    };
    let mut failures = Vec::new();
    for (name, GithubRepo(market)) in &m.claude.marketplaces {
        if name == PROFILE_MARKETPLACE {
            continue;
        }
        add(market.clone(), reads);
        let wanted: Vec<&str> = m
            .claude
            .plugins
            .iter()
            .filter(|p| &p.marketplace == name)
            .map(|p| p.name.as_str())
            .collect();
        if wanted.is_empty() {
            continue;
        }
        let sources = match cx
            .forge()
            .read_file_at(market, None, MARKETPLACE_FILE, token)
        {
            Ok((_, Some(json))) => profile::plugin_sources(&json, name, &wanted),
            Ok((_, None)) => Err(format!(
                "marketplace {name}: {market} has no {MARKETPLACE_FILE}"
            )),
            Err(e) => Err(format!("marketplace {name}: {market}: {e}")),
        };
        match sources {
            Ok(repos) => repos.into_iter().for_each(|r| add(r, reads)),
            Err(e) => failures.push((ProfileStep::Manifest, e)),
        }
    }
    failures
}

/// Records what `gathered` found on Box `id`: the profile, its events and a warning per failure.
///
/// # Errors
/// [`CommandError`] when a store write fails.
pub(crate) fn record(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    gathered: Gathered,
) -> Result<(), CommandError> {
    if let Some(p) = &gathered.resolved {
        store.set_box_profile(id, p)?;
        store.apply(Decision::new(id).event(ExtraEvent::ProfileResolved {
            repo: p.repo.clone(),
            sha: p.sha.as_str().to_owned(),
        }))?;
    }
    for (step, detail) in gathered.failures {
        failed(cx, store, id, step, detail)?;
    }
    Ok(())
}
