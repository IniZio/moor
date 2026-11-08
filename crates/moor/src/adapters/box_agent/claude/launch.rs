//! Claude launch: argv, hook files and the merge steps that run on the Box.

use moor_core::boxscript::quote;
use moor_core::name::{BoxPath, NameError};
use moor_core::seam::agent_adapter::{BoxFile, BoxSetup, EnvVar, Key, LaunchCtx};
use moor_core::seam::engine::LaunchPrompt;
use moor_core::Secret;
use serde_json::{json, Value};

use self::hook_script::HOOK_SCRIPT;
pub(super) use self::preseed::preseed;

mod hook_script;
mod mcp;
mod preseed;
#[cfg(test)]
mod tests;

/// Tools that ask the user a question.
const ASK_MATCHER: &str = "AskUserQuestion|ExitPlanMode";

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

/// Arguments for a fresh session, ending in `--` and the first prompt unless the launch asks for none.
pub(super) fn argv(ctx: &LaunchCtx) -> Vec<String> {
    let mut argv = flags(ctx);
    let prompt = match &ctx.prompt {
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

/// The instructions, model and permission flags shared by both argument lists.
fn flags(ctx: &LaunchCtx) -> Vec<String> {
    let mut flags = vec![
        "--append-system-prompt-file".to_owned(),
        ctx.instructions.as_str().to_owned(),
    ];
    if let Some(model) = &ctx.model {
        flags.extend(["--model".to_owned(), model.clone()]);
    }
    if bypasses_permissions(ctx) {
        flags.extend(["--permission-mode".to_owned(), BYPASS_MODE.to_owned()]);
    }
    if !ctx.mcp.is_empty() {
        flags.extend(["--mcp-config".to_owned(), mcp::config(ctx).to_string()]);
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
pub(super) fn parent(path: &str) -> String {
    path.rsplit_once('/').map_or("/", |(dir, _)| dir).to_owned()
}

/// Shell that runs the python or node form of a script, else `fallback`; fails when none can.
pub(super) fn interpreters(py: &str, js: &str, args: &str, fallback: &str) -> String {
    format!(
        "if command -v python3 >/dev/null 2>&1; then python3 -c {py} {args}; \
elif command -v node >/dev/null 2>&1; then node -e {js} {args}; \
elif {fallback}; then :; else false; fi",
        py = quote(py),
        js = quote(js),
    )
}

/// The settings fragment: the hooks and the approval of the project's MCP servers, whose screen
/// would otherwise hold the start, plus the bypass-mode acceptance when the agent launches so.
fn fragment_json(ctx: &LaunchCtx, script: &str) -> Value {
    let mut fragment = hooks(script);
    if let Some(map) = fragment.as_object_mut() {
        map.insert("enableAllProjectMcpServers".to_owned(), true.into());
        if bypasses_permissions(ctx) {
            map.insert("skipDangerousModePermissionPrompt".to_owned(), true.into());
        }
    }
    fragment
}

/// The hook entries to merge, each running the hook script with its event.
fn hooks(script: &str) -> Value {
    let hook = |event: &str, matcher: Option<&str>| {
        let command = format!("sh {} {event}", quote(script));
        let mut entry = json!({ "hooks": [{ "type": "command", "command": command }] });
        if let Some(matcher) = matcher {
            if let Some(map) = entry.as_object_mut() {
                map.insert("matcher".to_owned(), matcher.into());
            }
        }
        entry
    };
    json!({ "hooks": {
        "UserPromptSubmit": [hook("UserPromptSubmit", None)],
        "Stop": [hook("Stop", None)],
        "SessionStart": [hook("SessionStart", Some("resume"))],
        "PreToolUse": [hook("PreToolUse", Some(ASK_MATCHER))],
    }})
}
