#!/usr/bin/env bash
# Fails when a removed identifier, the intent pid, or a removed run flag reappears.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
. "$here/lib/repo-root.sh"
# MOOR_SCAN_ROOT points the scan at another tree (the lint selftest); default is the repo root.
if [ -n "${MOOR_SCAN_ROOT:-}" ]; then cd "$MOOR_SCAN_ROOT"; else cd_repo_root; fi
names=(
  proxy_gate MOOR_PROXY_UPSTREAM MOOR_GH_API MOOR_GH_REPO MOOR_PUSH_URL
  PROXY_SERVED gh_shim model_route ModelRoute ANTHROPIC_BASE_URL insteadOf
  RouteRow RouteTable WebConfig web_feed start_web
  CredentialSource GithubConfig GithubApp you_token_cmd DEFAULT_YOU_TOKEN_CMD pass_env
  not_gh_token proxied_claude_var
  check_bound check_unbound same_path WORKTREE_OPERATOR_ONLY insert_unnamed repo_seq
  open_boxes_by_repo open_box_by_path
  ResyncGates rerun_sync sync_name resync_skipped resync_failed no_sync
  will_launch agent_live
  SyncMode sync_checkout sync_with_proxy sync_failed seed_source CRABBOX_SYNC_GIT_SEED_SOURCE
  push_url git_remote_lines MOOR_FETCH remote.moor
  host_cwd host_of 'box_dir(row' 'box_dir(&row'
  create_worktree remove_worktree list_worktrees existing_worktree WorktreeEntry worktree_created
  'Worktree {' 'surface::Worktree' Leftover exclude_scratch SCRATCH_DIRS
  UnpushedWork worktree_kept sync_branch BranchSync 'worktree TEXT' 'row.worktree'
  InReq split_dir in_dir with_dir caller_dir IN_OPERATOR_ONLY ReposReq ReposValue RepoView
  REPOS_OPERATOR_ONLY RepoEntry RepoTable MoorYamlSource find_repo resolve_repos known_repos
  repo_cache repo_overlay overlay_config known_repo trust_moor_yaml settings_at_base load_repo
  main_repo_config main_checkout fetch_default fetch_branch clone_repo commit_of file_at
  'Identity::of(cx' 'Layer::Git' 'proxy::origin'
  DirectHost check_direct direct_hosts RepoProxy WaitingForBox '--pool-max' provider_of
  seal_script seal_caches seal_step cache_seal_failed work_tree_not_kept MOOR_SEAL_BASE OnFail
  moor_yaml moorYaml
  MOOR_CRABBOX_BIN MOOR_TRACE_CRABBOX link_dial ProviderReady provider_ready
  PoolHosts PoolPorts HostLists
  reverse_tunnel local_tunnel TunnelHandle LocalTunnel TunnelFailure require_reverse_tunnel tend_tunnel
  SleepKind hide_unsupported_sleep lookup_timeout_secs
  moor-herdr moor_herdr MOOR_HERDR_BIN MOOR_HERDR_TAG MOOR_HERDR_RELEASES HERDR_PLUGIN_STATE_DIR
  'Tool::Herdr' moor-hc moor_hc
  REPORT_OUTPUT_BRIEF
)
# Plain identifiers match as whole words, so `in_dir` does not hit `builtin_direct`.
# Entries holding punctuation or spaces cannot be whole words; they stay fixed substrings.
word_args=(); sub_args=()
for n in "${names[@]}"; do
  if [[ "$n" =~ [^A-Za-z0-9_] ]]; then sub_args+=(-e "$n"); else word_args+=(-e "$n"); fi
done
scan=(-rn --include='*.rs' --include='*.sh'
  --exclude='tests.rs' --exclude-dir=tests --exclude-dir=target)
# insteadOf was retired with the old Proxy listener; only the submodule fetch's per-command ssh
# rewrite may use it, and it writes no git config.
hits=$( { grep "${scan[@]}" -wF "${word_args[@]}" crates/ || true
          grep "${scan[@]}" -F "${sub_args[@]}" crates/ || true; } | sort -u |
  grep -vE '^crates/moor-core/src/boxscript/clone\.rs:[0-9]+:.*-c url\.https://github\.com/\.insteadOf=' || true )
if [ -n "$hits" ]; then
  echo "removed-names: removed names found"
  echo "$hits"
  echo "fix: the name was retired; use what replaced it (git log -S'<name>' shows the change), or pick a new name"
  exit 1
fi

# One write stays, never a read: a rolled-back binary refuses an intent with no pid until the
# column is dropped. Its test reads the column back as that binary would.
pid_hits=$(grep -rnw --include='*.rs' --exclude-dir=target -e intent_pid -e IntentPid crates/ \
  | grep -v '^crates/moor-schema/' \
  | grep -vE '^crates/moor/src/store/apply\.rs:[0-9]+:        \(BoxState::IntentPid, pid\.into\(\)\),$' \
  | grep -v '^crates/moor/src/store/tests/rollback_pid\.rs:' || true)
if [ -n "$pid_hits" ]; then
  echo "removed-names: the intent pid is read or written"
  echo "$pid_hits"
  echo "fix: a Box's owner says whether an op is in flight; use Owners::busy"
  exit 1
fi

texts=()
for t in crates docs ./*.md; do [ -e "$t" ] && texts+=("${t#./}"); done
flag_hits=$(grep -rnIF -e '--no-sync' -e '--sync' -e '--trust-moor-yaml' \
  --exclude-dir=target "${texts[@]}" || true)
if [ -n "$flag_hits" ]; then
  echo "removed-names: removed run flags found"
  echo "$flag_hits"
  echo "fix: these run flags were removed; drop them from the docs and code"
  exit 1
fi

echo "removed-names: ok"
