//! Item assembly for `env`, in a fixed order.

use std::path::Path;

use moor_core::config::egress::{unenforced_note, EgressItem};
use moor_core::config::{DotfilesConfig, Identity, Layer, TaskSettings};
use moor_core::seam::driver::EgressSupport;
use moor_core::view::env::{EnvGithub, EnvItem, EnvKind, EnvMint, EnvResolved};

use crate::cx::Cx;
use crate::ops::common::creds::{checked, plain, Checked};

/// All items in display order.
pub(crate) fn assemble(
    cx: &Cx,
    s: &TaskSettings,
    worktree: &Path,
    egress: &EgressItem,
    support: EgressSupport,
) -> Vec<EnvItem> {
    let mut items = Vec::new();
    if let Ok(author) = cx.git().author(worktree) {
        for (name, val) in [("user.name", author.name), ("user.email", author.email)] {
            if let Some(v) = val {
                items.push(EnvItem {
                    source: Layer::Git,
                    ..plain(EnvKind::Identity, name, &v)
                });
            }
        }
    }
    let Checked { credentials, mcp } = checked(cx, s, worktree);
    items.extend(credentials);
    items.extend(s.setup.iter().map(|t| plain(EnvKind::Setup, t, t)));
    items.extend(mcp);

    let provider = s.provider.value.as_str();
    items.push(plain(EnvKind::Egress, egress.name, &egress.detail));
    if let Some(note) = unenforced_note(support, s.enforce_egress) {
        items.push(plain(EnvKind::Egress, "enforce", note));
    }
    items.push(EnvItem {
        source: s.provider.source,
        ..plain(EnvKind::Box, "provider", provider)
    });
    items.push(EnvItem {
        source: s.size.source,
        ..plain(EnvKind::Box, "size", s.size.value.as_str())
    });
    items.push(EnvItem {
        delivery: Some("proxy".to_owned()),
        ..plain(EnvKind::Box, "gh", "proxy shim (pr create|view|list)")
    });
    if let Some(d) = &s.dotfiles {
        items.push(dotfiles_item(cx, d));
    }
    items
}

/// Dotfiles row.
fn dotfiles_item(cx: &Cx, d: &DotfilesConfig) -> EnvItem {
    let install = d.install.as_deref().map_or_else(
        || "install: install.sh if present".to_owned(),
        |s| format!("install: {s}"),
    );
    let ok = cx.creds().env().expand_home(&d.path).is_dir();
    EnvItem {
        source: Layer::User,
        resolves: Some(ok),
        error: (!ok).then(|| "dotfiles path not found".to_owned()),
        ..plain(
            EnvKind::Dotfiles,
            "path",
            &format!("{}; {install}", d.path.display()),
        )
    }
}

/// GitHub block. No App store exists yet, so no App is ever set up.
pub(crate) fn github(s: &TaskSettings) -> EnvGithub {
    let (mode, token_source) = match s.identity {
        Identity::Bot => ("bot", "GitHub App installation token"),
        Identity::You => ("app-you", "GitHub App user token (repo-scoped)"),
        Identity::Auto => ("gh-you", "gh CLI (gh auth token)"),
    };
    EnvGithub {
        identity: s.identity.as_str().to_owned(),
        app_id: None,
        login: if s.identity == Identity::Bot {
            "n/a"
        } else {
            "logged_out"
        }
        .to_owned(),
        mint: EnvMint {
            status: "skipped".to_owned(),
            error: None,
            reason: Some("GitHub App not set up".to_owned()),
        },
        resolved: EnvResolved {
            mode: Some(mode.to_owned()),
            app_installed: "n/a".to_owned(),
            token_source: Some(token_source.to_owned()),
            error: None,
        },
    }
}
