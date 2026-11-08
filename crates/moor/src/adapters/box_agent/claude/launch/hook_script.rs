//! The script Claude Code's hooks run in the Box.

/// Writes `agent-state.json` beside itself: `hook.sh STATE EVENT`, hook JSON on stdin.
/// `ts` is when it ran and `turn` when the prompt of the current turn was submitted, both in
/// milliseconds on the Box's clock; every report but a prompt's carries `turn` forward.
/// The read is capped in size and time, so a closed or silent stdin never stalls the agent.
/// A Stop's last message is rebuilt from the transcript's final assistant reply, as Claude Code's
/// `last_assistant_message` holds only the reply's last text block; a transcript whose reply does
/// not end with that block has not caught up, and the payload's text stands.
/// A question tool's record keeps its questions, and no Notification replaces it: Claude Code
/// follows the question with a generic permission notice that names nothing it asked.
/// Each state change is also printed to `MOOR_AGENT_TTY` as an OSC 7501 sequence, once per change:
/// `osc` holds the one mapping from hook state to terminal state. The launch script runs it with
/// `done SessionEnd` when the agent exits, which prints without touching the record.
/// `background_local` says every piece of background work is a local shell or subagent, the only
/// kinds whose end the Box can see; remote or MCP work runs on with nothing to show for it.
/// A busy record keeps the tool's name and, cut short, the input fields the `doing` line
/// reads; never the tool's output or any other input.
pub(super) const HOOK_SCRIPT: &str = r#"#!/bin/sh
d=${0%/*}
osc() {
  [ -n "$MOOR_AGENT_TTY" ] && [ -w "$MOOR_AGENT_TTY" ] || return 0
  case $1:$2 in
    waiting_input:PreToolUse) s=blocked:kind=question ;;
    waiting_input:*) s=blocked:kind=permission ;;
    working:*|idle:*|done:*) s=$1 ;;
    *) return 0 ;;
  esac
  p="state=$s:app=claude"
  [ "$(cat "$d/status-osc" 2>/dev/null)" = "$p" ] && return 0
  printf %s "$p" > "$d/status-osc" 2>/dev/null
  printf '\033]7501;%s\033\\' "$p" >> "$MOOR_AGENT_TTY" 2>/dev/null || :
}
if [ "$2" = SessionEnd ]; then osc "$1" "$2"; exit 0; fi
in=
if command -v timeout >/dev/null 2>&1; then in=$(timeout 2 head -c 65536 2>/dev/null); fi
if [ "$2" = Notification ] && grep -q '"state": *"waiting_input", *"event": *"PreToolUse"' "$d/agent-state.json" 2>/dev/null; then exit 0; fi
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
o = {"state": sys.argv[1], "event": sys.argv[2], "ts": int(time.time() * 1000)}
for k in ("session_id", "message", "notification_type", "last_assistant_message"):
    if isinstance(d.get(k), str):
        o[k] = d[k]
if sys.argv[2] == "Stop" and isinstance(d.get("transcript_path"), str):
    try:
        with open(d["transcript_path"], "rb") as f:
            f.seek(0, 2)
            f.seek(max(0, f.tell() - 1048576))
            lines = f.read().decode("utf-8", "replace").splitlines()
        texts = []
        for line in lines:
            try:
                e = json.loads(line)
            except ValueError:
                continue
            if not isinstance(e, dict) or e.get("isSidechain"):
                continue
            m = e.get("message")
            if e.get("type") == "user":
                texts = []
            elif e.get("type") == "assistant" and isinstance(m, dict) and isinstance(m.get("content"), list):
                texts += [b["text"] for b in m["content"] if isinstance(b, dict) and b.get("type") == "text" and isinstance(b.get("text"), str) and b["text"].strip()]
        whole = "\n\n".join(texts)
        last = o.get("last_assistant_message")
        if whole and (last is None or whole.rstrip().endswith(last.strip())):
            o["last_assistant_message"] = whole
    except Exception:
        pass
bg = d.get("background_tasks")
if isinstance(bg, list):
    run = [t for t in bg if isinstance(t, dict) and t.get("status", "running") == "running"]
    names = [str(t.get("description") or t.get("type") or t.get("id") or "")[:200] for t in run]
    o["background"] = [n for n in names if n][:10]
    o["background_local"] = all(t.get("type") in ("shell", "subagent") for t in run)
try:
    p = json.load(open(sys.argv[3] + "/agent-state.json"))
    if not isinstance(p, dict):
        p = {}
except (OSError, ValueError):
    p = {}
if sys.argv[2] == "UserPromptSubmit":
    o["turn"] = o["ts"]
elif isinstance(p.get("turn"), int):
    o["turn"] = p["turn"]
if sys.argv[2] == "Notification" and "last_assistant_message" not in o:
    if p.get("state") == "idle" and isinstance(p.get("last_assistant_message"), str):
        o["last_assistant_message"] = p["last_assistant_message"]
    if p.get("state") == "idle" and isinstance(p.get("background"), list):
        o["background"] = p["background"]
        o["background_local"] = p.get("background_local") is True
ti = d.get("tool_input")
if isinstance(ti, dict) and isinstance(ti.get("questions"), list):
    o["questions"] = ti["questions"]
if sys.argv[1] == "working" and isinstance(d.get("tool_name"), str):
    o["tool"] = d["tool_name"][:100]
    if isinstance(ti, dict):
        o["tool_input"] = {k: ti[k][:300] for k in ("command", "file_path", "notebook_path", "url") if isinstance(ti.get(k), str)}
print(json.dumps(o))' "$1" "$2" "$d"
elif command -v node >/dev/null 2>&1; then
  MOOR_HOOK_IN=$in node -e '
let d = {};
try { d = JSON.parse(process.env.MOOR_HOOK_IN || ""); } catch (e) {}
if (d === null || typeof d !== "object" || Array.isArray(d)) d = {};
const o = { state: process.argv[1], event: process.argv[2], ts: Date.now() };
for (const k of ["session_id", "message", "notification_type", "last_assistant_message"]) {
  if (typeof d[k] === "string") o[k] = d[k];
}
if (process.argv[2] === "Stop" && typeof d.transcript_path === "string") {
  try {
    const fs = require("fs");
    const fd = fs.openSync(d.transcript_path, "r");
    const size = fs.fstatSync(fd).size;
    const buf = Buffer.alloc(Math.min(size, 1048576));
    fs.readSync(fd, buf, 0, buf.length, size - buf.length);
    fs.closeSync(fd);
    let texts = [];
    for (const line of buf.toString("utf8").split("\n")) {
      let e;
      try { e = JSON.parse(line); } catch (err) { continue; }
      if (!e || typeof e !== "object" || e.isSidechain) continue;
      const m = e.message;
      if (e.type === "user") texts = [];
      else if (e.type === "assistant" && m && Array.isArray(m.content)) {
        for (const b of m.content) if (b && b.type === "text" && typeof b.text === "string" && b.text.trim()) texts.push(b.text);
      }
    }
    const whole = texts.join("\n\n");
    const last = o.last_assistant_message;
    if (whole && (last === undefined || whole.trimEnd().endsWith(last.trim()))) o.last_assistant_message = whole;
  } catch (err) {}
}
if (Array.isArray(d.background_tasks)) {
  const run = d.background_tasks
    .filter((t) => t && typeof t === "object" && (t.status === undefined || t.status === "running"));
  o.background_local = run.every((t) => t.type === "shell" || t.type === "subagent");
  o.background = run
    .map((t) => String(t.description || t.type || t.id || "").slice(0, 200))
    .filter((n) => n)
    .slice(0, 10);
}
let p = {};
try { p = JSON.parse(require("fs").readFileSync(process.argv[3] + "/agent-state.json", "utf8")); } catch (e) {}
if (p === null || typeof p !== "object") p = {};
if (process.argv[2] === "UserPromptSubmit") o.turn = o.ts;
else if (Number.isInteger(p.turn)) o.turn = p.turn;
if (process.argv[2] === "Notification" && !("last_assistant_message" in o)) {
  if (p.state === "idle" && typeof p.last_assistant_message === "string") o.last_assistant_message = p.last_assistant_message;
  if (p.state === "idle" && Array.isArray(p.background)) {
    o.background = p.background;
    o.background_local = p.background_local === true;
  }
}
const ti = d.tool_input;
if (ti && typeof ti === "object" && Array.isArray(ti.questions)) o.questions = ti.questions;
if (process.argv[1] === "working" && typeof d.tool_name === "string") {
  o.tool = d.tool_name.slice(0, 100);
  if (ti && typeof ti === "object") {
    o.tool_input = {};
    for (const k of ["command", "file_path", "notebook_path", "url"]) if (typeof ti[k] === "string") o.tool_input[k] = ti[k].slice(0, 300);
  }
}
console.log(JSON.stringify(o));' "$1" "$2" "$d"
else
  ts=$(date +%s%3N 2>/dev/null)
  case $ts in ''|*[!0-9]*) ts=$(($(date +%s) * 1000)) ;; esac
  if [ "$2" = UserPromptSubmit ]; then turn=$ts
  else turn=$(sed -n 's/.*"turn": *\([0-9][0-9]*\).*/\1/p' "$d/agent-state.json" 2>/dev/null); fi
  printf '{"state":"%s","event":"%s","ts":%s,"turn":%s}\n' "$1" "$2" "$ts" "${turn:-null}"
fi > "$t" 2>/dev/null && mv -f "$t" "$d/agent-state.json" || rm -f "$t"
osc "$1" "$2"
exit 0
"#;
