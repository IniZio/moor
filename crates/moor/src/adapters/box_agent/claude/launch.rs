//! Claude launch: argv, hook files and the merge steps that run on the Box.

use moor_core::boxscript::quote;
use moor_core::name::{BoxPath, NameError};
use moor_core::seam::agent_profile::{BoxFile, BoxSetup, EnvVar, Key, LaunchCtx};
use moor_core::seam::engine::LaunchPrompt;
use moor_core::Secret;
use serde_json::{json, Value};

#[cfg(test)]
mod tests;

/// The prompt that points the agent at its brief.
const PROMPT: &str = "Read and follow .moor/brief.md";
/// Notification kinds that mean the agent waits on the user.
const NOTIFY_MATCHER: &str =
    "permission_prompt|elicitation_dialog|elicitation_url_dialog|agent_needs_input";
/// Tools that ask the user a question.
const ASK_MATCHER: &str = "AskUserQuestion|ExitPlanMode";

/// Writes `agent-state.json` beside itself: `hook.sh STATE EVENT`, hook JSON on stdin.
/// The read is capped in size and time, so a closed or silent stdin never stalls the agent.
const HOOK_SCRIPT: &str = r#"#!/bin/sh
d=${0%/*}
in=
if command -v timeout >/dev/null 2>&1; then in=$(timeout 2 head -c 65536 2>/dev/null); fi
t="$d/agent-state.json.tmp.$$"
if command -v python3 >/dev/null 2>&1; then
  MOOR_HOOK_IN=$in python3 -c '
import json, os, sys, time
try:
    d = json.loads(os.environ.get("MOOR_HOOK_IN", ""))
except ValueError:
    d = {}
if not isinstance(d, dict):
    d = {}
o = {"state": sys.argv[1], "event": sys.argv[2], "ts": int(time.time())}
for k in ("session_id", "message", "notification_type"):
    if isinstance(d.get(k), str):
        o[k] = d[k]
print(json.dumps(o))' "$1" "$2"
elif command -v node >/dev/null 2>&1; then
  MOOR_HOOK_IN=$in node -e '
let d = {};
try { d = JSON.parse(process.env.MOOR_HOOK_IN || ""); } catch (e) {}
if (d === null || typeof d !== "object" || Array.isArray(d)) d = {};
const o = { state: process.argv[1], event: process.argv[2], ts: Math.floor(Date.now() / 1000) };
for (const k of ["session_id", "message", "notification_type"]) {
  if (typeof d[k] === "string") o[k] = d[k];
}
console.log(JSON.stringify(o));' "$1" "$2"
else
  printf '{"state":"%s","event":"%s","ts":%s}\n' "$1" "$2" "$(date +%s)"
fi > "$t" 2>/dev/null && mv -f "$t" "$d/agent-state.json" || rm -f "$t"
exit 0
"#;

/// Adds the fragment's hooks and other settings to the settings file, dropping earlier entries that
/// run the hook script.
const MERGE_HOOKS_PY: &str = r#"
import json, os, sys
p, f, m = sys.argv[1:4]
try:
    j = json.load(open(p))
except FileNotFoundError:
    j = {}
hooks = j.setdefault("hooks", {})
frag = json.load(open(f))
for k, v in frag.items():
    if k != "hooks":
        j[k] = v
for ev, new in frag["hooks"].items():
    keep = [e for e in hooks.get(ev, []) if not any(m in str(h.get("command", "")) for h in e.get("hooks", []))]
    hooks[ev] = keep + new
os.makedirs(os.path.dirname(p), exist_ok=True)
t = p + ".moor-tmp"
with os.fdopen(os.open(t, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), "w") as o:
    json.dump(j, o, indent=2)
os.replace(t, p)
"#;

/// The node form of [`MERGE_HOOKS_PY`].
const MERGE_HOOKS_JS: &str = r#"
const fs = require("fs"), path = require("path");
const [p, f, m] = process.argv.slice(1);
let j = {};
try { j = JSON.parse(fs.readFileSync(p, "utf8")); } catch (e) { if (e.code !== "ENOENT") throw e; }
j.hooks = j.hooks || {};
const frag = JSON.parse(fs.readFileSync(f, "utf8"));
for (const [k, v] of Object.entries(frag)) if (k !== "hooks") j[k] = v;
for (const [ev, add] of Object.entries(frag.hooks)) {
  const keep = (j.hooks[ev] || []).filter(e => !(e.hooks || []).some(h => String(h.command || "").includes(m)));
  j.hooks[ev] = keep.concat(add);
}
fs.mkdirSync(path.dirname(p), { recursive: true });
const t = p + ".moor-tmp";
fs.writeFileSync(t, JSON.stringify(j, null, 2), { mode: 0o600 });
fs.renameSync(t, p);
"#;

/// Sets onboarding, theme and folder trust in `~/.claude.json`, keeping other keys.
const PRESEED_PY: &str = r#"
import json, os, sys
p, w = sys.argv[1:3]
try:
    j = json.load(open(p))
except FileNotFoundError:
    j = {}
j["hasCompletedOnboarding"] = True
j["theme"] = "dark"
j.setdefault("projects", {}).setdefault(w, {})["hasTrustDialogAccepted"] = True
t = p + ".moor-tmp"
with os.fdopen(os.open(t, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), "w") as o:
    json.dump(j, o, indent=2)
os.replace(t, p)
"#;

/// The node form of [`PRESEED_PY`].
const PRESEED_JS: &str = r#"
const fs = require("fs");
const [p, w] = process.argv.slice(1);
let j = {};
try { j = JSON.parse(fs.readFileSync(p, "utf8")); } catch (e) { if (e.code !== "ENOENT") throw e; }
j.hasCompletedOnboarding = true;
j.theme = "dark";
j.projects = j.projects || {};
j.projects[w] = Object.assign({}, j.projects[w], { hasTrustDialogAccepted: true });
const t = p + ".moor-tmp";
fs.writeFileSync(t, JSON.stringify(j, null, 2), { mode: 0o600 });
fs.renameSync(t, p);
"#;

/// Arguments for a fresh session, ending in `--` and the first prompt unless the launch asks for none.
pub(super) fn argv(ctx: &LaunchCtx) -> Vec<String> {
    let mut argv = flags(ctx);
    let prompt = match &ctx.prompt {
        LaunchPrompt::Brief => PROMPT,
        LaunchPrompt::Text(text) => text,
        LaunchPrompt::None => return argv,
    };
    argv.extend(["--".to_owned(), prompt.to_owned()]);
    argv
}

/// Arguments that continue a stored session.
pub(super) fn resume_argv(session_id: &str, ctx: &LaunchCtx) -> Vec<String> {
    let mut argv = vec!["--resume".to_owned(), session_id.to_owned()];
    argv.extend(flags(ctx));
    argv
}

/// The permission mode of an agent that runs behind a Proxy.
const BYPASS_MODE: &str = "bypassPermissions";

/// Whether the agent launches in [`BYPASS_MODE`]; its confirmation screen is then pre-accepted in
/// the settings the hook writer merges.
const fn bypasses_permissions(ctx: &LaunchCtx) -> bool {
    ctx.proxy.is_some()
}

/// The model and permission flags shared by both argument lists.
fn flags(ctx: &LaunchCtx) -> Vec<String> {
    let mut flags = Vec::new();
    if let Some(model) = &ctx.model {
        flags.extend(["--model".to_owned(), model.clone()]);
    }
    if bypasses_permissions(ctx) {
        flags.extend(["--permission-mode".to_owned(), BYPASS_MODE.to_owned()]);
    }
    flags
}

/// Fixed non-secret token that keeps Claude Code from prompting for login; it matches the claude
/// tool rule's `box_env`, and the Proxy swaps in the real credential on the wire.
const OAUTH_PLACEHOLDER: &str = "moor-proxy";

/// Environment that stops Claude Code prompting for login; its traffic reaches the Proxy through
/// the Box's `HTTPS_PROXY`. Empty without a Proxy address.
pub(super) fn env(ctx: &LaunchCtx) -> Vec<EnvVar> {
    if ctx.proxy.is_none() {
        return Vec::new();
    }
    vec![EnvVar {
        name: "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
        value: Secret::new(OAUTH_PLACEHOLDER.as_bytes().to_vec()),
    }]
}

/// The digits of `n`.
pub(super) fn menu_keys(n: u8) -> Vec<Key> {
    n.to_string().chars().map(Key::Char).collect()
}

/// Merges first-run state into `~/.claude.json`; a failure only warns.
pub(super) fn preseed(ctx: &LaunchCtx) -> Result<BoxSetup, NameError> {
    let file = quote(BoxPath::under(ctx.home.as_str(), ".claude.json")?.as_str());
    let workdir = quote(ctx.workdir.as_str());
    let merge = interpreters(
        PRESEED_PY,
        PRESEED_JS,
        &format!("{file} {workdir}"),
        &format!(
            "[ ! -e {file} ] && (umask 077; printf '%s' {} > {file})",
            quote(&fresh_preseed(ctx))
        ),
    );
    Ok(BoxSetup {
        files: Vec::new(),
        step: format!(
            "{merge} || echo 'moor: warning: could not preseed ~/.claude.json' >&2; true"
        ),
    })
}

/// A whole `~/.claude.json` for a Box that has none.
fn fresh_preseed(ctx: &LaunchCtx) -> String {
    json!({
        "hasCompletedOnboarding": true,
        "theme": "dark",
        "projects": { ctx.workdir.as_str(): { "hasTrustDialogAccepted": true } },
    })
    .to_string()
}

/// Puts the hook script and hook fragment in `<workdir>/.moor` and merges the fragment into
/// `~/.claude/settings.json` in the Box agent's home, keeping other keys and hooks; running it
/// again changes nothing. In bypass mode the fragment also accepts Claude Code's confirmation
/// screen for that mode.
pub(super) fn hook_writer(ctx: &LaunchCtx) -> Result<BoxSetup, NameError> {
    let script = BoxPath::under(ctx.workdir.as_str(), ".moor/hook.sh")?;
    let fragment = BoxPath::under(ctx.workdir.as_str(), ".moor/hooks.json")?;
    let settings = BoxPath::under(ctx.home.as_str(), ".claude/settings.json")?;
    let args = format!(
        "{} {} {}",
        quote(settings.as_str()),
        quote(fragment.as_str()),
        quote(script.as_str())
    );
    let fresh = format!(
        "[ ! -e {s} ] && mkdir -p {d} && cp {f} {s}",
        s = quote(settings.as_str()),
        d = quote(&parent(settings.as_str())),
        f = quote(fragment.as_str())
    );
    Ok(BoxSetup {
        step: interpreters(MERGE_HOOKS_PY, MERGE_HOOKS_JS, &args, &fresh),
        files: vec![
            BoxFile {
                path: script.clone(),
                content: HOOK_SCRIPT.to_owned(),
                mode: 0o700,
            },
            BoxFile {
                path: fragment,
                content: fragment_json(ctx, script.as_str()).to_string(),
                mode: 0o600,
            },
        ],
    })
}

/// The directory part of an absolute path.
fn parent(path: &str) -> String {
    path.rsplit_once('/').map_or("/", |(dir, _)| dir).to_owned()
}

/// Shell that runs the python or node form of a script, else `fallback`; fails when none can.
fn interpreters(py: &str, js: &str, args: &str, fallback: &str) -> String {
    format!(
        "if command -v python3 >/dev/null 2>&1; then python3 -c {py} {args}; \
elif command -v node >/dev/null 2>&1; then node -e {js} {args}; \
elif {fallback}; then :; else false; fi",
        py = quote(py),
        js = quote(js),
    )
}

/// The settings fragment: the hooks, plus the bypass-mode acceptance when the agent launches so.
fn fragment_json(ctx: &LaunchCtx, script: &str) -> Value {
    let mut fragment = hooks(script);
    if bypasses_permissions(ctx) {
        if let Some(map) = fragment.as_object_mut() {
            map.insert("skipDangerousModePermissionPrompt".to_owned(), true.into());
        }
    }
    fragment
}

/// The hook entries to merge, each running the hook script with its state and event.
fn hooks(script: &str) -> Value {
    let hook = |state: &str, event: &str, matcher: Option<&str>| {
        let command = format!("sh {} {state} {event}", quote(script));
        let mut entry = json!({ "hooks": [{ "type": "command", "command": command }] });
        if let Some(matcher) = matcher {
            if let Some(map) = entry.as_object_mut() {
                map.insert("matcher".to_owned(), matcher.into());
            }
        }
        json!([entry])
    };
    json!({ "hooks": {
        "UserPromptSubmit": hook("working", "UserPromptSubmit", None),
        "Stop": hook("idle", "Stop", None),
        "SessionStart": hook("idle", "SessionStart", Some("resume")),
        "PostToolUse": hook("working", "PostToolUse", None),
        "Notification": hook("waiting_input", "Notification", Some(NOTIFY_MATCHER)),
        "PreToolUse": hook("waiting_input", "PreToolUse", Some(ASK_MATCHER)),
    }})
}
