# Sourced by lint scripts that list files through git: `. "$(dirname "$0")/lib/repo-root.sh"; cd_repo_root`.
# Exits non-zero with a message outside a git checkout (e.g. a `git archive` export) or when git lists no files,
# so a lint never reports ok after checking nothing.
cd_repo_root() {
  local top
  top=$(git rev-parse --show-toplevel 2>/dev/null) || {
    echo "$(basename "$0"): not a git checkout; cannot list files, nothing was checked" >&2
    exit 2
  }
  cd "$top"
  [ -n "$(git ls-files 2>/dev/null | head -n 1)" ] || {
    echo "$(basename "$0"): git ls-files returned no files; nothing was checked" >&2
    exit 2
  }
}
