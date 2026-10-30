//! The Claude Code profile writer: `~/.claude` from the person's profile and the repo's skills.

use moor_core::boxscript::quote;
use moor_core::name::{BoxPath, NameError};
use moor_core::profile::{Manifest, PROFILE_MARKETPLACE};
use moor_core::seam::agent_adapter::BoxSetup;
use moor_core::seam::proc::Tool;

use super::launch::{interpreters, parent};
use super::Claude;
use crate::adapters::box_agent::profile_writer::{
    ProfileCtx, ProfileWriter, FAILURES_FILE, FAIL_FN, MARKETPLACE_NAME_FN,
};

mod merge;
#[cfg(test)]
mod tests;

/// Copies each person skill, then links each repo skill over it of the same name, so the repo wins.
/// A directory without a `SKILL.md` is not a skill.
const SKILLS: &str = r#"for s in "$d"/skills/*/; do
  s=${s%/}; [ -f "$s/SKILL.md" ] || continue; n=${s##*/}
  rm -rf "$c/skills/$n"; cp -R "$s" "$c/skills/$n" || fail "skill $n" $?
done
for s in "$w"/.agents/skills/*/; do
  s=${s%/}; [ -f "$s/SKILL.md" ] || continue; n=${s##*/}
  rm -rf "$c/skills/$n"; ln -s "$s" "$c/skills/$n" || fail "skill $n" $?
done"#;

impl ProfileWriter for Claude {
    fn write(&self, ctx: &ProfileCtx<'_>) -> Result<BoxSetup, NameError> {
        let claude_dir = BoxPath::under(ctx.home.as_str(), ".claude")?;
        let settings = BoxPath::under(claude_dir.as_str(), "settings.json")?;
        let failures = BoxPath::under(ctx.workdir.as_str(), FAILURES_FILE)?;
        let vars = format!(
            "d={}; w={}; c={}; f={}",
            quote(ctx.profile_dir.as_str()),
            quote(ctx.workdir.as_str()),
            quote(claude_dir.as_str()),
            quote(failures.as_str()),
        );
        let setup = format!(
            "mkdir -p {} \"$c/skills\" && : > \"$f\"",
            quote(&parent(failures.as_str()))
        );
        let instructions = "[ -f \"$d/AGENTS.md\" ] && { cp \"$d/AGENTS.md\" \"$c/CLAUDE.md\" || fail instructions $?; }";
        let fragment = format!(
            "[ -f \"$d/claude/settings.json\" ] && {{ {} || fail settings $?; }}",
            interpreters(
                merge::MERGE_SETTINGS_PY,
                merge::MERGE_SETTINGS_JS,
                &format!("{} \"$d/claude/settings.json\"", quote(settings.as_str())),
                &format!(
                    "[ ! -e {s} ] && cp \"$d/claude/settings.json\" {s}",
                    s = quote(settings.as_str())
                ),
            )
        );
        let lines = [
            vars,
            FAIL_FN.to_owned(),
            setup,
            instructions.to_owned(),
            SKILLS.to_owned(),
            fragment,
            plugins(ctx.manifest),
        ];
        Ok(BoxSetup {
            files: Vec::new(),
            step: format!("{{\n{}\n}} || :", lines.join("\n")),
        })
    }
}

/// Adds each marketplace and installs each plugin; a failure is recorded and the rest still run.
fn plugins(manifest: &Manifest) -> String {
    let claude = Tool::Agent.program();
    let mut lines = Vec::new();
    for (name, repo) in &manifest.claude.marketplaces {
        let url = format!("https://github.com/{}", repo.0);
        lines.push(format!(
            "{claude} plugin marketplace add {} </dev/null >/dev/null 2>&1 || fail {} $?",
            quote(&url),
            quote(&format!("marketplace {name}")),
        ));
    }
    let uses_profile = manifest
        .claude
        .plugins
        .iter()
        .any(|p| p.marketplace == PROFILE_MARKETPLACE);
    if uses_profile {
        lines.push(MARKETPLACE_NAME_FN.to_owned());
        lines.push(format!(
            "pm=$(mkt_name \"$d/.claude-plugin/marketplace.json\") && [ -n \"$pm\" ] || pm=\n\
[ -n \"$pm\" ] && {{ {claude} plugin marketplace add \"$d\" </dev/null >/dev/null 2>&1 || fail {} $?; }}",
            quote(&format!("marketplace {PROFILE_MARKETPLACE}")),
        ));
    }
    for plugin in &manifest.claude.plugins {
        let id = format!("{}@{}", plugin.name, plugin.marketplace);
        let what = quote(&format!("plugin {id}"));
        if plugin.marketplace == PROFILE_MARKETPLACE {
            lines.push(format!(
                "if [ -n \"$pm\" ]; then {claude} plugin install {}\"@$pm\" </dev/null >/dev/null 2>&1 || fail {what} $?; else fail {what} 1; fi",
                quote(&plugin.name),
            ));
        } else {
            lines.push(format!(
                "{claude} plugin install {} </dev/null >/dev/null 2>&1 || fail {what} $?",
                quote(&id),
            ));
        }
    }
    lines.join("\n")
}
