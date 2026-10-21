//! Script for a recorded run: reset the Box checkout to fresh-checkout state, run setup, run the command.

use super::boxwork::{ignored_status_pattern, is_full_sha};
use super::quote;

/// Box-relative path of the file [`head_script`] writes.
pub const HEAD_FILE: &str = ".moor/record-head";

/// Refuse to run outside a git work tree.
const GIT_GUARD: &str = "git rev-parse --is-inside-work-tree >/dev/null 2>&1 || { echo 'moor: --record needs a git checkout on the Box' >&2; exit 1; }\n";

/// Files owned by another user (e.g. root, via a container bind mount) would
/// make the cache script's `rm -rf` fail and a matched cache would keep them.
/// Chown to us via passwordless sudo, else fail with a hint.
const OWNERSHIP_FIXUP: &str = r#"if [ -n "$(find . -xdev ! -user "$(id -u)" -print -quit 2>/dev/null)" ]; then
  if sudo -n true >/dev/null 2>&1; then
    sudo -n chown -R "$(id -u):$(id -g)" . || { echo 'moor: --record chown of root-owned files failed' >&2; exit 1; }
  else
    echo 'moor: --record found files not owned by you, likely root-owned; on the Box run: sudo chown -R "$(id -u):$(id -g)" .' >&2
    exit 1
  fi
fi
"#;

/// Decide which dependency dirs survive the clean; leaves the `-e` args in
/// the positional parameters. Node dirs also keep the self-validating tool
/// caches (`.next/cache`, `.turbo`, `.nx/cache`) under the same key; `.next`
/// itself is never kept. Lockfile paths come from repo content: filtered to
/// `[A-Za-z0-9._/@+-]`, never eval'd, always quoted.
const CACHE_SCRIPT: &str = r#"mkdir -p .moor/cache
rm -f .moor/cache/*.next
__moor_hash=
if command -v sha256sum >/dev/null 2>&1; then __moor_hash='sha256sum'
elif command -v shasum >/dev/null 2>&1; then __moor_hash='shasum -a 256'
elif command -v openssl >/dev/null 2>&1; then __moor_hash='openssl dgst -sha256'
fi
__moor_key() {
  { printf 'moor-cache-v1\n%s\n' "$2"; cat -- "$1"
    __d=$(dirname -- "$1")
    while :; do
      for __p in $3; do
        if [ -f "$__d/$__p" ]; then printf '\n%s/%s\n' "$__d" "$__p"; cat -- "$__d/$__p"; fi
      done
      [ "$__d" = . ] && break
      __d=$(dirname -- "$__d")
    done
    printf '\n'; uname -sm; printf '%s' "$__moor_setup"
  } | $__moor_hash
}
( LC_ALL=C; git ls-files | while IFS= read -r __f; do
    case "$__f" in node_modules/*|*/node_modules/*|vendor/*|*/vendor/*|Pods/*|*/Pods/*) continue ;; esac
    case "$__f" in *[!A-Za-z0-9._/@+-]*|*..*) continue ;; esac
    case "${__f##*/}" in
      package-lock.json|pnpm-lock.yaml|yarn.lock|bun.lock|bun.lockb|uv.lock|poetry.lock|Pipfile.lock|Gemfile.lock|composer.lock|Podfile.lock) printf '%s\n' "$__f" ;;
    esac
  done ) > .moor/cache/.locks
__moor_ok=1
if [ -z "$__moor_hash" ]; then
  echo 'moor: warning: --record cannot keep dependency dirs: no sha256sum, shasum or openssl on the Box' >&2
  __moor_ok=
elif [ "$(wc -l < .moor/cache/.locks | tr -d ' ')" -gt 64 ]; then
  echo 'moor: warning: --record cannot keep dependency dirs: more than 64 lockfiles' >&2
  __moor_ok=
fi
set --
if [ -n "$__moor_ok" ]; then
  while IFS= read -r __f; do
    __nest=
    case "${__f##*/}" in
      package-lock.json|pnpm-lock.yaml|yarn.lock|bun.lock|bun.lockb) __eco=node; __pins='.nvmrc .node-version .tool-versions .mise.toml mise.toml'; __dir=node_modules; __nest=1 ;;
      uv.lock|poetry.lock|Pipfile.lock) __eco=python; __pins='.python-version .tool-versions .mise.toml mise.toml'; __dir=.venv ;;
      Gemfile.lock) __eco=ruby; __pins='.ruby-version .tool-versions .mise.toml mise.toml'; __dir=vendor/bundle ;;
      composer.lock) __eco=php; __pins='.tool-versions .mise.toml mise.toml'; __dir=vendor ;;
      *) __eco=pods; __pins='.tool-versions .mise.toml mise.toml'; __dir=Pods ;;
    esac
    case "$__f" in */*) __pre="${__f%/*}/" ;; *) __pre= ;; esac
    __slug=$(printf '%s' "$__f" | sed 's,/,__,g')
    __key=$(__moor_key "$__f" "$__eco" "$__pins") || __key=
    if [ -n "$__key" ] && [ -f ".moor/cache/$__slug.key" ] && [ "$(cat ".moor/cache/$__slug.key")" = "$__key" ]; then
      set -- "$@" -e "/${__pre}${__dir}"
      if [ -n "$__nest" ]; then set -- "$@" -e "/${__pre}**/node_modules" -e "/${__pre}.next/cache" -e "/${__pre}**/.next/cache" -e "/${__pre}.turbo" -e "/${__pre}.nx/cache"; fi
    else
      rm -f ".moor/cache/$__slug.key"
      rm -rf -- "./${__pre}${__dir}"
      if [ -n "$__nest" ]; then find "./${__pre:-.}" -name node_modules -type d -prune -exec rm -rf {} +
        rm -rf -- "./${__pre}.turbo" "./${__pre}.nx/cache"
        find "./${__pre:-.}" -path '*/.next/cache' -type d -prune -exec rm -rf {} +
      fi
      printf '%s\n' "$__key" > ".moor/cache/$__slug.next"
    fi
  done < .moor/cache/.locks
fi
rm -f .moor/cache/.locks
"#;

/// Clean ignored and untracked files except `.moor/` and the kept dirs in
/// `$@`; on failure retry once after a passwordless-sudo chown.
const CLEAN: &str = r#"__moor_clean() { git clean -ffdxq -e '.moor/' "$@"; }
if ! __moor_clean "$@" 2>/dev/null; then
  if sudo -n true >/dev/null 2>&1; then
    sudo -n chown -R "$(id -u):$(id -g)" . && __moor_clean "$@" || { echo 'moor: --record clean failed after sudo chown' >&2; exit 1; }
  else
    echo 'moor: --record clean failed, likely root-owned files; on the Box run: sudo chown -R "$(id -u):$(id -g)" .' >&2
    exit 1
  fi
fi
set --
"#;

/// Run the prep in a child shell with errexit (`set -e` is ignored inside a
/// `||` context); a failure stops before the command.
const RUN_PREP: &str = r#"if command -v bash >/dev/null 2>&1; then bash -ec "$__moor_prep"; else sh -ec "$__moor_prep"; fi || { __moor_rc=$?; echo "moor: --record fresh-state prep failed (exit $__moor_rc)" >&2; exit $__moor_rc; }
"#;

/// Promote the pending cache stamps only when the command passed.
const PROMOTE: &str = r#"__moor_rc=$?
if [ "$__moor_rc" -eq 0 ]; then
  for __moor_n in .moor/cache/*.next; do
    [ -e "$__moor_n" ] || continue
    mv -f "$__moor_n" "${__moor_n%.next}.key"
  done
fi
exit "$__moor_rc"
"#;

/// POSIX sh text: fresh-checkout prep, `setup` steps (each its own `sh -c`,
/// first failure stops), then `command` (shell text); exits with the command's
/// status.
#[must_use]
pub fn record_script(setup: &[String], command: &str) -> String {
    let steps: String = setup
        .iter()
        .flat_map(|s| ["sh -c ", &quote(s), " || exit $?\n"].map(str::to_owned))
        .collect();
    let prep = [
        &format!("__moor_setup={}\n", quote(&setup.join("\n"))),
        OWNERSHIP_FIXUP,
        CACHE_SCRIPT,
        CLEAN,
        &steps,
    ]
    .concat();
    [
        GIT_GUARD,
        &format!("__moor_prep={}\n", quote(&prep)),
        RUN_PREP,
        "unset __moor_prep\n",
        &format!("( {command}\n)\n"),
        PROMOTE,
    ]
    .concat()
}

/// Script run with `sh -c` in the Box workdir before a record run: writes `head=SHA dirty=N` to
/// [`HEAD_FILE`], where `N` counts changed or untracked paths as the boxwork probe does.
#[must_use]
pub fn head_script() -> String {
    let ignored = quote(&ignored_status_pattern());
    format!(
        r#"export GIT_NO_LAZY_FETCH=1
mkdir -p .moor || exit 1
rm -f {HEAD_FILE}
head=$(git rev-parse --verify HEAD) || exit 1
st=$(git status --porcelain --no-renames) || exit 1
if [ -z "$st" ]; then
  dirty=0
else
  dirty=$(printf '%s\n' "$st" | grep -vcE {ignored} || true)
fi
printf 'head=%s dirty=%s\n' "$head" "$((dirty + 0))" > {HEAD_FILE} || exit 1
"#
    )
}

/// The HEAD sha and the dirty count from the text [`head_script`] wrote; `None` unless its
/// first line is exactly `head=SHA dirty=N` with a full sha.
#[must_use]
pub fn parse_head(text: &str) -> Option<(String, u32)> {
    let mut parts = text.lines().next()?.split_whitespace();
    let head = parts.next()?.strip_prefix("head=")?;
    let dirty = parts.next()?.strip_prefix("dirty=")?;
    if parts.next().is_some() || !is_full_sha(head) || !dirty.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((head.to_owned(), dirty.parse().ok()?))
}

#[cfg(test)]
mod tests;
