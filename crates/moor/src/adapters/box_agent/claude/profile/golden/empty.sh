{
d='/box/home/.moor-profile/person'; w='/box/work'; c='/box/home/.claude'; f='/box/work/.moor/profile-failures'
fail() { echo "moor: profile: $1 exit $2" >&2; printf '%s exit %s\n' "$1" "$2" >> "$f"; }
mkdir -p '/box/work/.moor' "$c/skills" && : > "$f"
[ -f "$d/AGENTS.md" ] && { cp "$d/AGENTS.md" "$c/CLAUDE.md" || fail instructions $?; }
for s in "$d"/skills/*/; do
  s=${s%/}; [ -f "$s/SKILL.md" ] || continue; n=${s##*/}
  rm -rf "$c/skills/$n"; cp -R "$s" "$c/skills/$n" || fail "skill $n" $?
done
for s in "$w"/.agents/skills/*/; do
  s=${s%/}; [ -f "$s/SKILL.md" ] || continue; n=${s##*/}
  rm -rf "$c/skills/$n"; ln -s "$s" "$c/skills/$n" || fail "skill $n" $?
done
[ -f "$d/claude/settings.json" ] && { if command -v python3 >/dev/null 2>&1; then python3 -c '
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
' '/box/home/.claude/settings.json' "$d/claude/settings.json"; elif command -v node >/dev/null 2>&1; then node -e '
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
' '/box/home/.claude/settings.json' "$d/claude/settings.json"; elif [ ! -e '/box/home/.claude/settings.json' ] && cp "$d/claude/settings.json" '/box/home/.claude/settings.json'; then :; else false; fi || fail settings $?; }

} || :