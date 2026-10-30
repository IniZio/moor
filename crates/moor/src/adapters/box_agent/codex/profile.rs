//! The Codex profile writer: `~/.codex` and `~/.agents/skills` from the person's profile.

use std::fmt::Write as _;

use moor_core::boxscript::quote;
use moor_core::name::{BoxPath, NameError};
use moor_core::profile::{Manifest, PROFILE_MARKETPLACE};
use moor_core::seam::agent_adapter::BoxSetup;

use super::Codex;
use crate::adapters::box_agent::profile_writer::{
    ProfileCtx, ProfileWriter, COPY_SKILLS, FAILURES_FILE, FAIL_FN,
};

#[cfg(test)]
mod tests;

/// Opens the block of top-level keys.
const BEGIN: &str = "# >>> moor-profile >>>";
/// Closes the block of top-level keys.
const END: &str = "# <<< moor-profile <<<";
/// Opens the block of tables.
const TABLES_BEGIN: &str = "# >>> moor-profile tables >>>";
/// Closes the block of tables.
const TABLES_END: &str = "# <<< moor-profile tables <<<";

/// Rewrites both fenced blocks of `$cfg`: the old ones go, everything else stays. TOML cannot
/// reopen the root table, so the top-level keys go first and the tables go last, where no
/// later key of the person's can fall into them.
const REPLACE_BLOCK: &str = r#"rest="$cfg.moor-rest"; : > "$rest"
[ -f "$cfg" ] && { awk -v b="$begin" -v e="$end" -v tb="$tbegin" -v te="$tend" '$0 == b || $0 == tb { skip = 1; next } $0 == e || $0 == te { skip = 0; next } !skip' "$cfg" > "$rest" || fail config $?; }
cat "$blk" "$rest" "$tblk" > "$cfg.moor-tmp" && mv "$cfg.moor-tmp" "$cfg" || fail config $?
rm -f "$rest" "$blk" "$tblk""#;

impl ProfileWriter for Codex {
    fn write(&self, ctx: &ProfileCtx<'_>) -> Result<BoxSetup, NameError> {
        let codex_dir = BoxPath::under(ctx.home.as_str(), ".codex")?;
        let agents_skills = BoxPath::under(ctx.home.as_str(), ".agents/skills")?;
        let failures = BoxPath::under(ctx.workdir.as_str(), FAILURES_FILE)?;
        let vars = format!(
            "d={}; c={}; k={}; f={}; cfg=\"$c/config.toml\"; begin={}; end={}; tbegin={}; tend={}",
            quote(ctx.profile_dir.as_str()),
            quote(codex_dir.as_str()),
            quote(agents_skills.as_str()),
            quote(failures.as_str()),
            quote(BEGIN),
            quote(END),
            quote(TABLES_BEGIN),
            quote(TABLES_END),
        );
        let setup = format!(
            "mkdir -p \"$c\" \"$k\" {} && : > \"$f\"",
            quote(
                failures
                    .as_str()
                    .rsplit_once('/')
                    .map_or("/", |(dir, _)| dir)
            ),
        );
        let instructions = "[ -f \"$d/AGENTS.md\" ] && { cp \"$d/AGENTS.md\" \"$c/AGENTS.md\" || fail instructions $?; }";
        let lines = [
            vars,
            FAIL_FN.to_owned(),
            setup,
            instructions.to_owned(),
            COPY_SKILLS.replace("$1", "$k"),
            config_block(ctx.manifest),
            REPLACE_BLOCK.to_owned(),
            plugins(ctx.manifest),
        ];
        Ok(BoxSetup {
            files: Vec::new(),
            step: format!("{{\n{}\n}} || :", lines.join("\n")),
        })
    }
}

/// Builds `$blk` (the profile's model, then the top-level keys of its `codex/config.toml`
/// fragment minus a `model` of its own, which would repeat the key) and `$tblk` (the fragment's
/// tables, from its first table header on).
fn config_block(manifest: &Manifest) -> String {
    let mut lines = vec![
        "blk=\"$cfg.moor-block\"; tblk=\"$cfg.moor-tables\"".to_owned(),
        "echo \"$begin\" > \"$blk\"; : > \"$tblk\"".to_owned(),
    ];
    let has_model = manifest.codex.model.is_some();
    if let Some(model) = &manifest.codex.model {
        lines.push(format!(
            "printf '%s\\n' {} >> \"$blk\"",
            quote(&format!("model = {}", toml_string(model)))
        ));
    }
    let top = if has_model {
        r#"awk '/^\[/ { t = 1 } !t && $0 !~ /^model[ \t]*=/' "$d/codex/config.toml""#
    } else {
        r#"awk '/^\[/ { t = 1 } !t' "$d/codex/config.toml""#
    };
    lines.push(format!(
        "[ -f \"$d/codex/config.toml\" ] && {{ {top} >> \"$blk\"; awk '/^\\[/ {{ t = 1 }} t' \"$d/codex/config.toml\" > \"$tblk\"; }}"
    ));
    lines.push("echo \"$end\" >> \"$blk\"".to_owned());
    lines.push(
        "[ -s \"$tblk\" ] && { { echo \"$tbegin\"; cat \"$tblk\"; echo \"$tend\"; } > \"$tblk.x\" && mv \"$tblk.x\" \"$tblk\"; }".to_owned(),
    );
    lines.join("\n")
}

/// A TOML basic string.
fn toml_string(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Adds each marketplace and installs each plugin through the Codex CLI, which clones the
/// snapshot and writes its own tables outside the block; a failure is recorded and the rest run.
fn plugins(manifest: &Manifest) -> String {
    let mut lines = Vec::new();
    for (name, repo) in &manifest.codex.marketplaces {
        let url = format!("https://github.com/{}", repo.0);
        lines.push(format!(
            "codex plugin marketplace add {} </dev/null >/dev/null 2>&1 || fail {} $?",
            quote(&url),
            quote(&format!("marketplace {name}")),
        ));
    }
    let uses_profile = manifest
        .codex
        .plugins
        .iter()
        .any(|p| p.marketplace == PROFILE_MARKETPLACE);
    if uses_profile {
        lines.push(
            r#"pm=$(codex plugin marketplace add --json "$d" </dev/null 2>/dev/null | sed -n 's/.*"marketplaceName": *"\([^"]*\)".*/\1/p') || pm=
[ -n "$pm" ] || fail 'marketplace profile' 1"#
                .to_owned(),
        );
    }
    for plugin in &manifest.codex.plugins {
        let id = format!("{}@{}", plugin.name, plugin.marketplace);
        let what = quote(&format!("plugin {id}"));
        if plugin.marketplace == PROFILE_MARKETPLACE {
            lines.push(format!(
                "if [ -n \"$pm\" ]; then codex plugin add {}\"@$pm\" </dev/null >/dev/null 2>&1 || fail {what} $?; else fail {what} 1; fi",
                quote(&plugin.name),
            ));
        } else {
            lines.push(format!(
                "codex plugin add {} </dev/null >/dev/null 2>&1 || fail {what} $?",
                quote(&id),
            ));
        }
    }
    lines.join("\n")
}
