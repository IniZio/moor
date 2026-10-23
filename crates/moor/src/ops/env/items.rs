//! Item assembly for `env`, in a fixed order.

use moor_core::config::{BoxSettings, DotfilesConfig, Layer};
use moor_core::seam::forge::{GitAuthor, RepoFiles};
use moor_core::view::env::{EnvItem, EnvKind};

use crate::cx::Cx;
use crate::ops::common::creds::{checked, plain, Checked};

/// All items in display order.
pub(crate) fn assemble(
    cx: &Cx,
    s: &BoxSettings,
    author: Option<&GitAuthor>,
    files: &RepoFiles,
) -> Vec<EnvItem> {
    let mut items = Vec::new();
    if let Some(author) = author {
        for (name, v) in [("user.name", &author.name), ("user.email", &author.email)] {
            items.push(EnvItem {
                source: Layer::Forge,
                ..plain(EnvKind::Identity, name, v)
            });
        }
    }
    let Checked { credentials, mcp } = checked(s, files);
    items.extend(credentials);
    items.extend(s.setup.iter().map(|t| plain(EnvKind::Setup, t, t)));
    items.extend(mcp);

    let (provider, source) = s
        .provider
        .as_ref()
        .map_or(("unset", Layer::Default), |p| (p.value.as_str(), p.source));
    items.push(EnvItem {
        source,
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
    let ok = cx.env().expand_home(&d.path).is_dir();
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
