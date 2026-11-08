//! The script Claude Code's hooks run in the Box.

/// Leaves what herdr-core cannot read off the terminal in its `moor-agent` doc:
/// `hook.sh EVENT`, hook JSON on stdin. Claude Code reports its own status to the terminal.
/// A prompt or a resume leaves only the session id; a Stop adds its last message and the
/// background work it left running; a question tool adds its questions.
/// The read is capped in size and time, so a closed or silent stdin never stalls the agent.
/// A Stop's last message is rebuilt from the transcript's final assistant reply, as Claude Code's
/// `last_assistant_message` holds only the reply's last text block; a transcript whose reply does
/// not end with that block has not caught up, and the payload's text stands.
/// `background_local` says every piece of background work is a local shell or subagent, the only
/// kinds whose end the Box can see; remote or MCP work runs on with nothing to show for it.
/// A Box without python3 or node leaves no note.
pub(super) const HOOK_SCRIPT: &str = r#"#!/bin/sh
in=
if command -v timeout >/dev/null 2>&1; then in=$(timeout 2 head -c 65536 2>/dev/null); fi
if command -v python3 >/dev/null 2>&1; then
  MOOR_HOOK_IN=$in python3 -c '
import json, os, subprocess, sys
try:
    d = json.loads(os.environ.get("MOOR_HOOK_IN", ""))
except ValueError:
    d = {}
if not isinstance(d, dict):
    d = {}
n = {}
if isinstance(d.get("session_id"), str):
    n["session_id"] = d["session_id"]
if sys.argv[1] == "Stop":
    said = d.get("last_assistant_message") if isinstance(d.get("last_assistant_message"), str) else None
    if isinstance(d.get("transcript_path"), str):
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
            if whole and (said is None or whole.rstrip().endswith(said.strip())):
                said = whole
        except Exception:
            pass
    if said is not None:
        n["said"] = said[:262144]
    bg = d.get("background_tasks")
    if isinstance(bg, list):
        run = [t for t in bg if isinstance(t, dict) and t.get("status", "running") == "running"]
        names = [str(t.get("description") or t.get("type") or t.get("id") or "")[:200] for t in run]
        n["background"] = [x for x in names if x][:10]
        n["background_local"] = all(t.get("type") in ("shell", "subagent") for t in run)
ti = d.get("tool_input")
if sys.argv[1] == "PreToolUse" and isinstance(ti, dict) and isinstance(ti.get("questions"), list):
    n["questions"] = ti["questions"]
def call(m, p):
    r = subprocess.run(["herdr-core", "connect"], input=json.dumps({"jsonrpc": "2.0", "id": 1, "method": m, "params": p}) + "\n", capture_output=True, text=True, timeout=5)
    return json.loads(r.stdout.splitlines()[0]).get("result")
try:
    for _ in range(3):
        v = call("doc.get", {"name": "moor-agent"})["version"]
        if call("doc.set", {"name": "moor-agent", "value": n, "version": v}) is not None:
            break
except Exception:
    pass' "$1"
elif command -v node >/dev/null 2>&1; then
  MOOR_HOOK_IN=$in node -e '
let d = {};
try { d = JSON.parse(process.env.MOOR_HOOK_IN || ""); } catch (e) {}
if (d === null || typeof d !== "object" || Array.isArray(d)) d = {};
const ev = process.argv[1];
const n = {};
if (typeof d.session_id === "string") n.session_id = d.session_id;
if (ev === "Stop") {
  let said = typeof d.last_assistant_message === "string" ? d.last_assistant_message : undefined;
  if (typeof d.transcript_path === "string") {
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
      if (whole && (said === undefined || whole.trimEnd().endsWith(said.trim()))) said = whole;
    } catch (err) {}
  }
  if (said !== undefined) n.said = said.slice(0, 262144);
  if (Array.isArray(d.background_tasks)) {
    const run = d.background_tasks
      .filter((t) => t && typeof t === "object" && (t.status === undefined || t.status === "running"));
    n.background = run
      .map((t) => String(t.description || t.type || t.id || "").slice(0, 200))
      .filter((x) => x)
      .slice(0, 10);
    n.background_local = run.every((t) => t.type === "shell" || t.type === "subagent");
  }
}
const ti = d.tool_input;
if (ev === "PreToolUse" && ti && typeof ti === "object" && Array.isArray(ti.questions)) n.questions = ti.questions;
const call = (m, p) => {
  const r = require("child_process").spawnSync("herdr-core", ["connect"], { input: JSON.stringify({ jsonrpc: "2.0", id: 1, method: m, params: p }) + "\n", timeout: 5000, encoding: "utf8" });
  return JSON.parse(String(r.stdout).split("\n")[0]).result;
};
try {
  for (let i = 0; i < 3; i++) {
    const v = call("doc.get", { name: "moor-agent" }).version;
    if (call("doc.set", { name: "moor-agent", value: n, version: v }) !== undefined) break;
  }
} catch (e) {}' "$1"
fi
exit 0
"#;
