# Live smoke runbook

Behaviour no fake can prove. Run before cutover with the rewritten `moor` binary, a cheap model (haiku) and a throwaway repo. `$REPOS` is the parent directory of local clones; `<repo>` is a disposable clone. Only delete Boxes this run created (`moor close`); never prune by pattern.

Every LIVE exclusion in `parity/exclusions.toml` has `ref = "live-smoke.md: <step>"` and its row id is listed under that step. A coverage check fails when a step name or a listed row is missing here.

## Namespace Box agent

Preconditions: provider `namespace-devbox`; `credentials.CLAUDE_CODE_OAUTH_TOKEN` set; Namespace CLI logged in.

Commands:
- `moor start live-ns --goal "run the unit tests and report" --quiet`
- `moor start live-ns-json --json --quiet` on a second repo (pure JSON expected)
- `moor pane live-ns --json` (reads the Box agent pane through the Surface)
- `moor run live-ns --record -- <compose job that leaves root-owned files>`, then `moor run live-ns -- true`
- plant a file named like the gate record on the Box, then `moor gate check <sha> live-ns`
- first `moor run` on a freshly created Box, repeated three times on three Boxes
- on a repo with a github.com origin, `moor start live-ns-pr --goal "create a pull request that adds one line to README"`; the agent runs `gh pr create` through the delivered shim without a permission prompt
- `moor close live-ns` and `moor close live-ns-pr`

Expected:
- `--quiet` prints exactly one start-log path on stderr and nothing else; with `--json` stdout stays pure JSON; warmup banner text never reaches stdout (B-71).
- `moor pane` decodes the agent pane (ANSI and JSON-envelope tolerant) and reports a state (A-42).
- a Box-local record file does not satisfy the gate; only the host-side record does (B-37).
- the Box agent opens the pull request unattended under `--dangerously-skip-permissions`: the Claude auto-mode classifier does not block `gh pr create`, the shim reaches only the Task repo through the proxy, and no GitHub or Claude token appears in the Box (C-120).
- the second `run` after a compose job with root-owned files still syncs and exits 0 (B-41).
- the first `run` on a fresh Box does not die with exit -1 "terminated signal received"; if it does, record the Box age and the crabbox log (B-17).

Security forwarding (old proxy against real GitHub; the parity suite proves it with a fake upstream, new binary only):
- C-70: from the Box, `git ls-remote` of the Task repo works; `git ls-remote` of any other repo through the proxy gets 403.
- C-71: `git fetch` and `git push` of the Task branch from the Box succeed; the Box holds no GitHub token and the push is attributed to the minted identity.
- C-72: `gh pr view` via the proxy works for 6+ minutes across a token refresh; no GitHub token appears in a Box file or env.
- C-73: `gh pr view` and `gh pr create` on the Task repo work; a REST call outside the allow-list (`DELETE /repos/<o>/<n>`) gets 403.
- C-74: `gh pr comment` on a pull request of the Task repo succeeds; a comment on a plain issue number gets 403.

Covers: A-42, B-17, B-37, B-41, B-71, C-70, C-71, C-72, C-73, C-74, C-120.

## static ssh host Box agent

Preconditions: static `ssh` provider pointing at a static ssh host you control; `box_credentials` listing one dummy secret; the repo clone present on that host.

Commands:
- `moor start live-ssh --goal "print the working directory and stop"`
- `moor run live-ssh -- sh -c 'command -v curl awk sed'`
- `moor close live-ssh`

Expected:
- the agent starts in the Driver-reported workdir; `brief.md` is delivered and removed from the credential directory after launch.
- `curl`, `awk` and `sed` exist with the flavours the `gh` shim and the proxy probe need; otherwise the probe error names curl.
- no listed credential value reaches the agent pane (`env | grep` in the pane is empty).
- close deletes the credential directory and leaves the static ssh host Box state unchanged (static host is not stopped).

Covers: no LIVE row; Box tool flavours (C gap 1) and the static-host Driver path.

## real Anthropic bearer

Preconditions: a real Claude subscription token as `credentials.CLAUDE_CODE_OAUTH_TOKEN`; Box agent Task started (Namespace or static ssh host step); Proxy running.

Commands:
- from the Box: `curl -sS -o /dev/null -w '%{http_code}\n' -H "anthropic-beta: oauth-2025-04-20" "$ANTHROPIC_BASE_URL/v1/models"`
- `moor proxy check live-ns --json`
- let the agent finish one turn

Expected:
- the Box holds only the per-Task nonce; the upstream receives the real bearer plus the `anthropic-beta` header and answers 200.
- `proxy check` reports the Claude route healthy; the token value appears in no Box file, argv or log.
- a wrong nonce gets 401, a non-allow-listed route 403.
- C-68: the real api.anthropic.com accepts the swapped bearer (200 on `/v1/models`); the nonce is never forwarded.

Covers: no LIVE row; C-68; C gap 1 (real api.anthropic.com acceptance, TLS and connection reuse).

## egress block

Preconditions: provider `sprites` (and a second run on `namespace-devbox` with `box.enforce_egress: true`); repo `egress: [example.com]`.

Commands:
- `moor start live-egress --goal "curl each host and report the status"`
- from the Box: `curl -sS -m 10 -o /dev/null -w '%{http_code}\n' https://example.com`, same for `https://api.github.com`, `https://api.anthropic.com`, `https://example.org`, `https://registry.npmjs.org`
- `moor env` before the start

Expected:
- `example.com` and `registry.npmjs.org` answer; `example.org` is blocked.
- `api.github.com` and `api.anthropic.com` are blocked directly (the Proxy serves them) while `github.com` release downloads work.
- the allow list is fixed at Box creation: editing `.moor.yaml egress` afterwards changes nothing on the running Box.
- on `local-container` and `ssh` the same curls all succeed (no policy applies).

Covers: no LIVE row; real enforcement behind B-99..B-102a and C-121..C-126; also the place to confirm C-126 once the override fix lands.

## tunnel RTT

Preconditions: Box agent Task on a remote provider; `crabbox tunnel --reverse` available in the installed crabbox.

Commands:
- start a Box agent Task; read the tunnel ports from `state/proxy/<task>.json`
- from the Box: `for i in 1 2 3 4 5; do curl -sS -o /dev/null -w '%{time_total}\n' "$ANTHROPIC_BASE_URL/healthz"; done`
- compare with the same request direct from the host

Expected:
- every Box request returns a status; the median extra latency stays within the accepted budget (about 0.5 s over direct, UL-69).
- killing the tunnel process makes the agent see connection errors, and `moor proxy check` reports the tunnel down rather than hanging.

- B-109 (old behaviour, note only): a tunnel that never prints its ready line is retried 5 times with 1,2,4,8 s backoff, then the Task logs `proxy_failed`; the parity suite proves the give-up with a test clock on the new binary only.

Covers: C-81, B-109, REQ-38; C gap 1 (reverse-tunnel RTT, connection reuse).
