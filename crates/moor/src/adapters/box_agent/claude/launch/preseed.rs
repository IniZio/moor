//! Preseeding `~/.claude.json` so the agent starts past its first-run screens.

use moor_core::boxscript::quote;
use moor_core::name::{BoxPath, NameError};
use moor_core::seam::agent_profile::{BoxSetup, LaunchCtx};
use serde_json::{json, Value};

use super::interpreters;

/// Sets onboarding, theme, folder trust and the external-imports notice in `~/.claude.json`,
/// keeping other keys. Completed onboarding skips the theme picker, the security notes and their
/// Enter prompt; the notice is marked shown without approving the imports. Trust goes on
/// the workdir, its resolved path and the git repository root above that, since Claude Code keys
/// trust by the physical cwd and accepts a trusted repository root for any folder inside it.
const PRESEED_PY: &str = r#"
import json, os, sys
p, w = sys.argv[1:3]
PROJECT = {"hasTrustDialogAccepted": True, "hasClaudeMdExternalIncludesWarningShown": True}
try:
    j = json.load(open(p))
except FileNotFoundError:
    j = {}
j["hasCompletedOnboarding"] = True
j["theme"] = "dark"
r = os.path.realpath(w)
g = r
while not os.path.exists(os.path.join(g, ".git")) and os.path.dirname(g) != g:
    g = os.path.dirname(g)
if not os.path.exists(os.path.join(g, ".git")):
    g = r
for k in {w, r, g}:
    j.setdefault("projects", {}).setdefault(k, {}).update(PROJECT)
t = p + ".moor-tmp"
with os.fdopen(os.open(t, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), "w") as o:
    json.dump(j, o, indent=2)
os.replace(t, p)
"#;

/// The node form of [`PRESEED_PY`].
const PRESEED_JS: &str = r#"
const fs = require("fs"), path = require("path");
const [p, w] = process.argv.slice(1);
let j = {};
try { j = JSON.parse(fs.readFileSync(p, "utf8")); } catch (e) { if (e.code !== "ENOENT") throw e; }
j.hasCompletedOnboarding = true;
j.theme = "dark";
j.projects = j.projects || {};
let r = w;
try { r = fs.realpathSync(w); } catch (e) {}
let g = r;
while (!fs.existsSync(path.join(g, ".git")) && path.dirname(g) !== g) g = path.dirname(g);
if (!fs.existsSync(path.join(g, ".git"))) g = r;
for (const k of new Set([w, r, g])) j.projects[k] = Object.assign({}, j.projects[k], { hasTrustDialogAccepted: true, hasClaudeMdExternalIncludesWarningShown: true });
const t = p + ".moor-tmp";
fs.writeFileSync(t, JSON.stringify(j, null, 2), { mode: 0o600 });
fs.renameSync(t, p);
"#;

/// Merges first-run state into `~/.claude.json`; the step fails when it cannot, since the agent
/// would then stop at a first-run screen no one is there to answer.
pub(in crate::adapters::box_agent::claude) fn preseed(ctx: &LaunchCtx) -> Result<BoxSetup, NameError> {
    let file = quote(BoxPath::under(ctx.home.as_str(), ".claude.json")?.as_str());
    let workdir = quote(ctx.workdir.as_str());
    let template = fresh_preseed(&[ctx.workdir.as_str(), RESOLVED, ROOT]).replace('%', "%%");
    let values = if template.find(RESOLVED) < template.find(ROOT) {
        "\"$r\" \"$g\""
    } else {
        "\"$g\" \"$r\""
    };
    let merge = interpreters(
        PRESEED_PY,
        PRESEED_JS,
        &format!("{file} {workdir}"),
        &format!(
            "( [ ! -e {file} ] || exit 1; umask 077; \
r=$(cd {workdir} 2>/dev/null && pwd -P) || r={workdir}; \
g=$r; while [ ! -e \"$g/.git\" ] && [ -n \"$g\" ]; do g=${{g%/*}}; done; \
[ -e \"$g/.git\" ] || g=$r; \
case \"$r$g\" in *'\"'*|*'\\'*) printf '%s' {plain} > {file} ;; \
*) printf {resolved} {values} > {file} ;; esac )",
            plain = quote(&fresh_preseed(&[ctx.workdir.as_str()])),
            resolved = quote(&template.replace(RESOLVED, "%s").replace(ROOT, "%s")),
        ),
    );
    Ok(BoxSetup {
        files: Vec::new(),
        step: format!(
            "{merge} || {{ echo 'moor: could not preseed ~/.claude.json; the agent would stop at its first-run screens' >&2; false; }}"
        ),
    })
}

/// Stands in for the resolved workdir, which only the Box knows, in [`fresh_preseed`].
const RESOLVED: &str = "MOOR_RESOLVED_WORKDIR";

/// Stands in for the git repository root above the resolved workdir in [`fresh_preseed`].
const ROOT: &str = "MOOR_REPOSITORY_ROOT";

/// A whole `~/.claude.json` for a Box that has none, trusting each of `dirs`.
fn fresh_preseed(dirs: &[&str]) -> String {
    let trusted: serde_json::Map<String, Value> = dirs
        .iter()
        .map(|d| {
            let project = json!({
                "hasTrustDialogAccepted": true,
                "hasClaudeMdExternalIncludesWarningShown": true,
            });
            ((*d).to_owned(), project)
        })
        .collect();
    json!({
        "hasCompletedOnboarding": true,
        "theme": "dark",
        "projects": trusted,
    })
    .to_string()
}
