#!/usr/bin/env bash
# Fails when code outside moor_core::exec_env assembles a Box exec environment.
set -euo pipefail
# Scans $1 (or $EXEC_ENV_ROOT) when given, so the checks can be self-tested; defaults to the repo's crates/.
if [ -n "${1:-${EXEC_ENV_ROOT:-}}" ]; then
  root="${1:-$EXEC_ENV_ROOT}"
else
  . "$(dirname "$0")/lib/repo-root.sh"
  cd_repo_root
  root=crates
fi
vars='HTTPS_PROXY|HTTP_PROXY|https_proxy|http_proxy|SSL_CERT_FILE|CURL_CA_BUNDLE|REQUESTS_CA_BUNDLE|GIT_SSL_CAINFO|NODE_EXTRA_CA_CERTS|NODE_USE_ENV_PROXY|GIT_CONFIG_COUNT|YARN_HTTPS_PROXY|YARN_HTTP_PROXY|YARN_CA_FILE_PATH|YARN_CACHE_FOLDER|YARN_GLOBAL_FOLDER|npm_config_cache'
files=$(find "$root" -name '*.rs' -not -path "$root/target/*" -not -path "$root/*/target/*" -not -path '*/tests/*' \
  -not -name tests.rs -not -path 'crates/moor-core/src/exec_env.rs' \
  -not -path 'crates/moor-core/src/exec_env/*' | sort)
# Exempt from (a): config/check.rs names the variables to refuse a repo `env:` that sets them, and
# brief.rs only mentions HTTPS_PROXY in prose to the agent; neither builds an env.
named=$(printf '%s\n' "$files" | grep -v -e '^crates/moor-core/src/config/check.rs$' -e '^crates/moor/src/adapters/box_agent/brief.rs$')
[ -n "$files" ] || { echo "exec-env: ok"; exit 0; }
# (a) proxy/CA variable names belong to the builder alone (comments ignored).
# shellcheck disable=SC2086
a=$(grep -nwE "$vars" $named /dev/null | grep -Ev "^[^:]*:[0-9]+:[[:space:]]*//" | sed 's/$/  <- proxy\/CA variable named outside exec_env/' || true)
# (b) an ExecSpec literal's env is exactly `env: Vec::new(),`, `env: vec![],`, `env: <path>.vars(),` or
# `env: <path>.vars.clone(),`; the `env,` shorthand and multi-line expressions are violations. The same
# forms are the only ones allowed in a one-line literal. A shell script cannot resolve types, so `<path>`
# is unchecked; (d) instead limits where a `vars` producer can be defined.
# shellcheck disable=SC2086
b=$(awk '
  FNR == 1 { depth = 0 }
  depth == 0 && /ExecSpec \{/ && !/struct ExecSpec/ { depth = 1; line = $0; sub(/.*ExecSpec \{/, "", line); depth += gsub(/\{/, "{", line) - gsub(/\}/, "}", line); 
    ok = "env: (Vec::new\\(\\)|vec!\\[\\]|[A-Za-z0-9_.]+\\.vars\\(\\)|[A-Za-z0-9_.]+\\.vars\\.clone\\(\\))[,} ]"
    gsub(ok, "", line)
    if (match(line, /(^|[^A-Za-z0-9_.])env([^A-Za-z0-9_]|$)/)) print FILENAME ":" FNR ":" $0 "  <- ExecSpec env not from exec_env"
    next }
  depth > 0 {
    if ($0 ~ /^[ \t]*env[ \t]*([:,}]|$)/ && $0 !~ /^[ \t]*env: (Vec::new\(\)|vec!\[\]|[A-Za-z0-9_.]+\.vars\(\)|[A-Za-z0-9_.]+\.vars\.clone\(\)),[ \t]*$/)
      print FILENAME ":" FNR ":" $0 "  <- ExecSpec env not from exec_env"
    t = $0; depth += gsub(/\{/, "{", t) - gsub(/\}/, "}", t)
  }' $files || true)
# (c) no `.env = `, `.env.push(` or `.env.extend(` on an ExecSpec after construction. proc.rs is exempt:
# its `ProcSpec::env` builder sets a host-side process env, not a Box exec env.
# shellcheck disable=SC2086
c=$(grep -nE '\.env( =|\.push\(|\.extend\()' $files /dev/null | grep -Ev "^[^:]*:[0-9]+:[[:space:]]*//" \
  | grep -v '^crates/moor-core/src/seam/proc.rs:' | sed 's/$/  <- ExecSpec env mutated after construction/' || true)
# (d) `vars` producers live in exec_env.rs (ExecEnv.vars) and box_exec.rs (BoxExec::vars) only.
# shellcheck disable=SC2086
d=$(grep -nE 'fn vars\(' $files /dev/null | grep -v '^crates/moor/src/ops/common/box_exec.rs:' \
  | sed 's/$/  <- vars producer outside exec_env\/box_exec/' || true)
hits=$(printf '%s\n%s\n%s\n%s\n' "$a" "$b" "$c" "$d" | sed '/^$/d')
if [ -n "$hits" ]; then
  echo "exec-env: Box exec env built outside moor_core::exec_env"
  echo "$hits"
  exit 1
fi
echo "exec-env: ok"
