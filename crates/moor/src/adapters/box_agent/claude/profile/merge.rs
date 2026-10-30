//! Merging the profile's settings fragment into `~/.claude/settings.json`.

/// Adds the fragment to the settings file: objects merge key by key, lists gain the entries they
/// lack, and a value already in the settings stays, so moor's own hooks and keys win.
pub(super) const MERGE_SETTINGS_PY: &str = r#"
import json, os, sys
p, f = sys.argv[1:3]
def merge(a, b):
    if isinstance(a, dict) and isinstance(b, dict):
        for k, v in b.items():
            a[k] = merge(a[k], v) if k in a else v
        return a
    if isinstance(a, list) and isinstance(b, list):
        return a + [x for x in b if x not in a]
    return a
try:
    j = json.load(open(p))
except FileNotFoundError:
    j = {}
j = merge(j, json.load(open(f)))
os.makedirs(os.path.dirname(p), exist_ok=True)
t = p + ".moor-tmp"
with os.fdopen(os.open(t, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), "w") as o:
    json.dump(j, o, indent=2)
os.replace(t, p)
"#;

/// The node form of [`MERGE_SETTINGS_PY`].
pub(super) const MERGE_SETTINGS_JS: &str = r#"
const fs = require("fs"), path = require("path");
const [p, f] = process.argv.slice(1);
const isObj = x => x !== null && typeof x === "object" && !Array.isArray(x);
function merge(a, b) {
  if (isObj(a) && isObj(b)) {
    for (const [k, v] of Object.entries(b)) a[k] = k in a ? merge(a[k], v) : v;
    return a;
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    const seen = new Set(a.map(x => JSON.stringify(x)));
    return a.concat(b.filter(x => !seen.has(JSON.stringify(x))));
  }
  return a;
}
let j = {};
try { j = JSON.parse(fs.readFileSync(p, "utf8")); } catch (e) { if (e.code !== "ENOENT") throw e; }
j = merge(j, JSON.parse(fs.readFileSync(f, "utf8")));
fs.mkdirSync(path.dirname(p), { recursive: true });
const t = p + ".moor-tmp";
fs.writeFileSync(t, JSON.stringify(j, null, 2), { mode: 0o600 });
fs.renameSync(t, p);
"#;
