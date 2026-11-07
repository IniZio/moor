//! The requester's agent profile, resolved through the forge before the Box exists.
//!
//! No profile step ever fails the start: each failure becomes a `profile_failed` event and a
//! warning, and the Box starts without what the step would have given it.

use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::{ProfileStep, ResolvedSource};
use moor_core::profile::{self, MANIFEST_FILE};
use moor_core::seam::engine::GitSource;
use moor_core::user::UserId;
use moor_core::{BoxId, Secret, Warning};

use super::repo::github_token;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::profile_fetch::failed;
use crate::store::box_profile::BoxProfile;
use crate::store::Store;

/// The path a lookup of a commit asks for, which no repo holds, so only the commit comes back.
pub(crate) const SOURCE_LOOKUP: &str = ".moor-no-such-file";

/// How long a start waits for the commits of the repos a profile installs from.
const LOOKUP_BOUND: Duration = Duration::from_secs(5);

/// What the forge said about the requester's profile.
pub(crate) struct Gathered {
    /// The profile at its resolved commit; `None` when the ref did not resolve.
    resolved: Option<BoxProfile>,
    /// The commit each repo it installs from resolved to.
    sources: Vec<ResolvedSource>,
    /// Why a repo it installs from did not resolve, one per repo.
    unresolved: Vec<String>,
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
        sources: Vec::new(),
        unresolved: Vec::new(),
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
    let mut listed = Vec::new();
    match manifest.as_deref().map(profile::parse) {
        Some(Err(e)) => failures.push((ProfileStep::Manifest, e.to_string())),
        Some(Ok(parsed)) => listed = cx.engine().profile_sources(&parsed),
        None => {}
    }
    let (sources, unresolved) = resolve_sources(cx, listed, token, LOOKUP_BOUND);
    let resolved = BoxProfile {
        repo: source.repo,
        sha,
        manifest,
    };
    Ok(Some(Gathered {
        resolved: Some(resolved),
        sources,
        unresolved,
        failures,
    }))
}

/// The commit of each of `listed`, and why for each that has none. A ref that is a whole commit id
/// needs no lookup. The rest are asked at once, and one still unanswered at `bound` counts as
/// unresolved, so a slow forge costs a Box its checkpoint and not its start.
pub(crate) fn resolve_sources(
    cx: &Cx,
    listed: Vec<GitSource>,
    token: Secret,
    bound: Duration,
) -> (Vec<ResolvedSource>, Vec<String>) {
    let token = Arc::new(token);
    let (mut sources, mut unresolved) = (Vec::new(), Vec::new());
    let (tx, rx) = mpsc::channel();
    let mut waiting = Vec::new();
    for source in listed {
        let GitSource { url, git_ref } = source;
        if let Some(sha) = git_ref.as_deref().filter(|r| is_commit(r)) {
            let sha = sha.to_owned();
            sources.push(ResolvedSource { url, sha });
            continue;
        }
        let Some(repo) = cx.forge().repo_of(&url) else {
            unresolved.push(format!("{url}: not on this forge"));
            continue;
        };
        waiting.push(url.clone());
        let (forge, token, tx) = (cx.shared_forge(), token.clone(), tx.clone());
        thread::spawn(move || {
            let at = git_ref.as_deref();
            let found = forge.read_file_at(&repo, at, SOURCE_LOOKUP, &token);
            let _ = tx.send((url, found.map(|(sha, _)| sha)));
        });
    }
    let by = cx.clock().instant() + bound;
    while !waiting.is_empty() {
        let left = by.saturating_duration_since(cx.clock().instant());
        let Ok((url, found)) = rx.recv_timeout(left) else {
            break;
        };
        if let Some(at) = waiting.iter().position(|w| *w == url) {
            waiting.remove(at);
        }
        match found {
            Ok(sha) => sources.push(ResolvedSource {
                url,
                sha: sha.as_str().to_owned(),
            }),
            Err(e) => unresolved.push(format!("{url}: {e}")),
        }
    }
    let late = waiting
        .into_iter()
        .map(|url| format!("{url}: no answer in {} s", bound.as_secs_f32()));
    unresolved.extend(late);
    (sources, unresolved)
}

/// Whether `git_ref` is a whole commit id, which needs no lookup.
fn is_commit(git_ref: &str) -> bool {
    git_ref.len() == 40 && git_ref.bytes().all(|b| b.is_ascii_hexdigit())
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
            sources: gathered.sources,
            unresolved: gathered.unresolved.clone(),
        }))?;
        for why in gathered.unresolved {
            cx.warn(Warning {
                code: "profile_source_unresolved".to_owned(),
                message: format!(
                    "could not tell what your profile would install, so this Box installs it \
                     again: {why}"
                ),
            });
        }
    }
    for (step, detail) in gathered.failures {
        failed(cx, store, id, step, detail)?;
    }
    Ok(())
}
