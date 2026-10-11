# Area D: Reports, agent-facing contract, lessons

Source: frozen moor `$REPOS/moor` main b8e8801 (read-only). Files read: `src/report.rs`, `src/brief.rs`, `src/cli.rs`, `src/pane.rs`, `src/registry.rs` (reports table), `plugins/herdr/**`, `skills/moor/SKILL.md`, `README.md`, `docs/adr/0003`, `tests/task_cli.rs` (report tests), `.groundwork/work/v1/{motive.md,evidence/t07,evidence/t09,evidence/c4/usage-log.md}`, `.groundwork/BACKLOG.md`.

Conventions. Coverage: BB = black-box testable through CLI + fake externals; INT = internal/unit only; BB+ = BB plus a live-only residue. UL-n = usage-log item n. The log reuses numbers: sections are first-run (1-57), drive 2 (57-76), drive 3 (1-15), 3b (16-29), 3c (30-37), 3d (38-43), 3e (44-54), 3f (55-61), 3g (62-71), 3h (72-80), 3i (81-84), 3j (85-88), 3j-b (89-99). Where a number is ambiguous the drive is given: `UL-57(d2)`. D# = motive.md decision.

Findings on scope: there is NO top-level `moor setup`, `moor doctor` or `moor status`. "Setup" = `moor github setup` (cli.rs `GithubCmd::Setup`). The doctor-like surfaces are `moor env` (credential/ref resolution + GitHub mint check) and `moor proxy check`. `moor doctor` (crabbox-runtime pack check) is only in BACKLOG, never built. `sleep`/`wake` subcommands do not exist (UL-94).

## 1. D-rows: Report behaviours

| ID | Given / When / Then (observable) | Cov | Flags |
|---|---|---|---|
| D-01 | Open Task t1; `report t1 --kind spec --file a.md --json` -> row `{id,task_id,kind,version:1,backend:"local",link:"file://<abs>",title,ts}`; link target is a copy under `<state>/reports/<task_id>/<kind>/v<N>.<ext>` (ext of source; none if none) with identical bytes; exit 0 | BB | path layout is state-dir internal but tests assert it (`assert_local_copy`); rewrite may relocate, keep "link resolves to a copy" |
| D-02 | Same, second `--file` same kind -> version 2 (auto `MAX+1` per Task+kind); first copy unchanged (editing source after does not change v1) | BB | |
| D-03 | Kinds are independent version counters (`spec` v1, `result` v1) | BB | |
| D-04 | `--link <url>` -> backend `agentic-artifacts`, link stored verbatim, no copy; version = `--version N` else next | BB | backend names `local` / `agentic-artifacts` are config values users write |
| D-05 | `--link` with `--version 3` then `--link` without -> v4 (next after max) | BB | |
| D-06 | Duplicate (task, kind, version) -> non-zero exit, nothing recorded (UNIQUE) | BB | |
| D-07 | `--version` < 1 -> error `report version must be >= 1, got N` | INT | |
| D-08 | Blank/whitespace `--link` -> error `report link is blank` | INT | |
| D-09 | Kind must match `[A-Za-z0-9_-]{1,64}`; else error `invalid report kind ...` (so `a/b` rejected, no path traversal) | BB | security-relevant: kind becomes a dir name |
| D-10 | Unknown Task, or Task not open (closed/rejected) -> error `no open task <name>`, exit 1, nothing written | BB | |
| D-11 | Empty `--title ""` stored as null; non-empty stored | BB | |
| D-12 | Clap rules: `--file` xor `--link` required; `--version` requires `--link`, conflicts `--file` | BB | exit 2 usage error |
| D-13 | Source file missing/unreadable -> error `copy <src> to <dest>`; no row | BB | |
| D-14 | Local row insert fails after copy -> copied file removed (no orphan) | INT | |
| D-15 | Dest `v<N>.<ext>` already exists on disk -> error `report file ... already exists` (no overwrite) | INT | |
| D-16 | Success appends event `reported {kind,version,backend,link,title}` (event id order; `list.last_event.kind == "reported"`) | BB | |
| D-17 | Reporting never changes Task `state` (stays `working`, etc.); agent state unaffected (ADR 0003, D7) | BB | core invariant |
| D-18 | Human output: `reported <task> <kind> v<N>  <link>`; `--json` pretty row | BB | human text not contractual |
| D-19 | `reports <task> [--json]` -> array of ALL versions ordered by row id, same row shape; works for closed Tasks (open Task of that name, else latest Task with that name); unknown name -> `no task <name>` exit 1 | BB | asymmetry: `report` needs OPEN task, `reports` reads closed |
| D-20 | Human `reports`: `<kind> v<N>  <backend>  <link>  <title or ->` per line | BB | |
| D-21 | `list --json` rows carry `reports: [ReportRow]`; human `list` col `reports <count> <last link>` or `-` | BB | |
| D-22 | `pane <task> --json` = list row + `events[]` + `transcript`; human shows `reports` block: `title  kind  version  url  link` per row or `(none)` | BB | herdr pane consumer |
| D-23 | Duplicate-name Tasks: `pane` picks latest non-closed/non-rejected, else latest | BB | |
| D-24 | Registry reports survive close (record stays); Report files stay in state dir after close/remove | BB | |
| D-25 | User config `reports.backend: local|agentic-artifacts` (default local); unknown backend or unknown key under `reports` -> config error | BB | |
| D-26 | DROPPED: decision 46, no host agent. Host-agent brief, backend local: tells agent to write `.moor/reports/<kind>.md` (git-excluded, not committed) then run `<moor> report <task> --kind <k> --file .moor/reports/<k>.md --title "<t>"`; exact text pinned by `host_brief_unchanged` | BB | brief text is a contract with the agent, not byte-exact for rewrite |
| D-27 | DROPPED: decision 47, artifacts backend dropped. Host-agent brief, backend agentic-artifacts: sha256sum -> MCP `publish_artifact` (project, name `<task>/<kind>`, size_bytes, content_hash, content_type text/markdown, metadata title) -> `curl PUT` upload_url -> `commit_artifact` -> `<moor> report <task> --kind <k> --link <viewer_url> --version <version_num> --title`; fall back to local if publish fails and say so | BB | dispatched agent gets the MCP tool allowed (`mcp__agentic-artifacts__publish_artifact` in the claude argv); moor holds no artifacts credential |
| D-28 | Box-agent brief: Reports written to `.moor/reports/<kind>.md` in Box workdir only; names the path in final message; NO moor command, no host path, no `--state-dir` (pinned by `box_brief_has_no_moor_command_or_host_path`) | BB | UL-19 |
| D-29 | Box Report reaches registry only via host: `cp <task> .moor/reports/<k>.md <host>` then `report --file` (verified UL-30(3c) h) | BB | front-door blocker (D28) |
| D-30 | The Box brief only (decision 46): a Report never means finished; keep working; same-kind again = new version | BB | |
| D-31 | `--state-dir` global (hidden) pins registry+state for this call; no commands are embedded in a brief (decision 46) (old test `state_dir_flag_pins_registry_and_agent_commands`) | BB | old brief embedded `<moor> --state-dir <dir> report ...`; dropped with the host brief (decision 46) |
| D-32 | Comment relay from Report viewers to agent: NOT built (BACKLOG a) | n/a | do not port as implied |

### Herdr plugin (V09, `plugins/herdr`)

| ID | Given / When / Then | Cov | Flags |
|---|---|---|---|
| D-40 | Plugin is stateless: shells `moor` + `jq`; `MOOR_BIN` from `$(herdr plugin config-dir moor)/env` (shell KEY=VALUE), else `moor` on PATH; missing jq or moor -> message and pause | BB (fake herdr/moor) | min herdr 0.7.0; linux+macos |
| D-41 | Task lookup order: `MOOR_TASK` env; Workspace label `moor:<task>`; else `moor list --json` filtered `state` not closed/rejected where `workspace_id == $HERDR_WORKSPACE_ID` or `worktree == checkout_path/workspace_cwd`, first hit by `.name` | BB | consumes list fields `name,state,workspace_id,worktree` |
| D-42 | Task pane (split): loops `moor pane <task> --events ${MOOR_PANE_EVENTS:-10}` every `${MOOR_PANE_INTERVAL:-5}`s, clears screen, prints `refreshed HH:MM:SS, every Ns`; `MOOR_PANE_ONCE=1` exits after one render; no task -> help text | BB | human `pane` text is the plugin UI contract (name/state header, issue, box, agent, worktree, transcript, reports, events) |
| D-43 | Pane refresh shows state transitions with no action (started, checked, box_ready, session_started, steered, ran, agent done) | BB+ | V09 live |
| D-44 | Action `steer`: prompt `Message`; empty = cancelled; `moor steer <task> <msg> --json`; success prints `delivered to session <.session.session_id>`; failure shows stderr; pauses for Enter | BB | |
| D-45 | Action `close`: requires typing the task name; runs `moor close <task> --json` DETACHED (own session via `setsid -w` or perl `POSIX::setsid` on macOS) because closing removes the Workspace and kills the plugin pane and the moor process in it; prints `closed <t>; Box deleted=<.box_deleted>; checkout kept (moor remove <t> deletes it)` | BB+ | V09 bug: close in-pane left Task `working`, no `closed` event. REQ-40 |
| D-46 | Action `shell`: `moor shell <task>`; exit 0 closes pane, nonzero pauses | BB | |
| D-47 | Action `start-here`: refuses if Workspace already has a Task; needs linked git worktree (`worktree.is_linked_worktree == true`); path = `worktree.checkout_path` else `workspace_cwd`; default name = branch minus `moor/`, `/`->`-`; prompts Task name, Goal (empty = no agent), Issue; runs `moor start <name> [--goal g] [--issue i] --worktree <path> --json`; prints `<name> <state> workspace <workspace>` + `reason` | BB | V09 found original refused non-`moor/*` branches; now binds any branch (D11, UL-3). Workspace relabelled `moor:<task>` |
| D-48 | Five entrypoints (task, steer, close, shell, start-here) opened via `herdr plugin pane open --plugin moor --entrypoint E --placement split|tab [--target-pane P --direction right | --workspace W] --focus`; actions `task-pane, steer, shell, close, start-here` in workspace context | BB (fake herdr) | |
| D-49 | No remove action (BACKLOG) | n/a | gap, not behaviour |
| D-50 | Every action failure pauses (`Press Enter to close.`) so the user can read it | BB | |

## 2. Agent contract (what agents/peers may rely on)

Consumers: ORCH = orchestrating (host) agent via skill; BOX = in-Box agent via brief; HOST = dispatched host agent via brief; PLUG = herdr plugin; HOOK = repo git/Claude hooks; HUMAN.

| Capability | Current command / JSON / exit | Consumers |
|---|---|---|
| Discoverability | SKILL description triggers: "local port already allocated", "dev stack busy", "work needs verifying/running on a Box", start/attach/list/close/remove a task, inspect merged env (UL-1) | ORCH |
| Start/attach (idempotent) | `start <task> [--issue][--goal][--model][--worktree P][--quiet][--agent-location host|box] --json` -> `{attached,task:Task,reason?,warnings?[],relaunched?{old_pane,new_pane,goal}}`; stdout pure JSON, chatter stderr; exit 1 when state `rejected`/`setup_failed` (Box acquire, dotfiles, credential mint) but outcome JSON still on stdout; failed repo `setup:` exits 0 with `warnings`; agent only dispatched with `--goal`; run from inside repo; fails `herdr is not available` without writing state | ORCH, PLUG |
| Task row | `{id,name,repo,issue,state,base,worktree,workspace,workspace_id,box:{driver,handle,state}|null,created_at,updated_at}`; Box state `starting|ready|failed|stopped|gone` (+ `release_failed`, UL-3j-b 7); handle opaque | ORCH, PLUG |
| Task name rule | `[a-z0-9-]`, <=40, not leading `-` | ORCH |
| Task states | checking, rejected, waiting_for_box, setting_up, working, setup_failed, idle, asleep, closed | ORCH, PLUG |
| List | `list --json` -> all Tasks (closed too) + `last_event{id,task_id,kind,payload,ts}|null`, `reports[]`, `agent|null {engine,session,state,location,pane_state?,waiting_for?,sessions,error?,exit_code?,agent_state?,agent_state_source?}`, `sleep?`; agent `state` working/blocked/done/failed/stopped/unknown | ORCH, PLUG |
| Pane | `pane <task> [--events N] [--json]` = list row + `events[]` + `transcript`; human text read by PLUG; exit 0; unknown `no task <n>` | PLUG, HUMAN |
| Steer | `steer <task> <msg> --json` -> `{task,session:{id,task_id,engine,session_id,cwd,state,created_at}}`; refuses when agent `blocked`; exited/gone pane -> error naming `moor start <task> --goal`; bare number answers AskUserQuestion menu; clears input line, types, submits (ctrl+m), one retry max; event `steered{submit_retried}`; wakes Box | ORCH, PLUG |
| Wait | `wait <task> [--timeout s] [--json]`; exit 124 on timeout else last run exit else 0; JSON `{timed_out, run, agent{agent_state,agent_state_source,state}}`; Box agent done when idle or blocked; source in hooks/hooks-cached/herdr/pane/timeout; timeout source counts busy, never idle; bound = timeout + ssh connect | ORCH |
| Run | `run <task> [--no-sync][--quiet][--record N] -- cmd...`: one arg = shell string (bash on Box, sh fallback), several = each shell-quoted; passthrough exit; 130 + `interrupted:true` when signal-killed; wakes asleep Box; event `ran{exit,sync,interrupted}`; errors `no ready Box`, `Box is gone; re-run moor start` | HOST, ORCH |
| Run --record | needs committed clean tree, refuses `--no-sync`; fresh state (git clean, setup re-run, lockfile-keyed dep dirs kept); nonzero prep => record exit; hint line on failure | ORCH, HOOK |
| Gate check | `gate check <sha> --name N [--command C] [--json]`: exit 0 pass, 1 no passing record (reason stderr), 2 bad/ambiguous sha; short sha >=4 hex via git else >=7 prefix match; any pass beats newer fail; JSON `{sha,name,command,exit,finished_at,task,ok}`; works from any dir | HOOK |
| Copy-back | `cp <task> <box-path> <host-path>`: no sync; relative box path = workdir; directory download replaces dest; needs ready Box; `chmod go-w` hint on group/world-writable config dir; event `copied` | ORCH, HUMAN |
| Open / shell | `open <task> --json` -> row + `woke`; `shell <task>` interactive, needs ready Box | HUMAN, PLUG |
| Close | `close <task> --json` -> row + `sessions_stopped[]`, `box_deleted`, `workspace_closed`; stops agents, saves transcript (box agent), runs `teardown:`, deletes Box, closes Workspace; never touches checkout; no push check | ORCH, PLUG |
| Remove | `remove <task> [--discard] --json` -> row + `worktree,worktree_removed,branch,branch_deleted,discarded`; closed only; refuses unpushed/uncommitted (lists them) and `--worktree`-bound checkouts | ORCH |
| Report / reports | see section 1 (D-01..D-32) | BOX(via host), HOST, ORCH |
| Env | `env [--json] [--agent-location host|box]` -> `{repo,items:[{kind,name,source,detail,resolves?,error?}], github:{identity,app_id,login,mint?}}`; kinds identity, credential, tool, setup, mcp, provision, egress, run, record, box, agent, dotfiles, `egress enforce`; mint check `github token: ok|failed:<chain>|skipped:<why>` | ORCH, HUMAN |
| GitHub setup | `github setup --app-id --client-id --key PATH(0600) [--repo] [--json]`; secret via `MOOR_GITHUB_CLIENT_SECRET` or stdin, never argv; saves then checks install (`App saved, but not installed on ...` error); `github login` device flow | HUMAN |
| Proxy check | `proxy check <task> [--json]`; exit 1 unless tunnel up; box request 403 = ok, 401 = nonce rejected, none = tunnel down; nonce never printed | ORCH, HUMAN |
| Version | `--version` (no commit hash: UL-89) | all |
| Errors | stderr `moor: <chain>`, exit 1; clap usage exit 2 | all |
| Events agents/peers key on | reported, session_started, steered, relaunched, ran, run_started, setup_failed{exit,step,log,box_log,tail}, setup_done, box_failed{stderr_tail}, box_ready, box_gone, dotfiles_applied/failed, agent_prompt_blocked, transcript_saved/failed{path,trigger}, teardown_*, proxy_started/tunnel_restarted/failed, github_identity, credential_failed(proposed UL-63), box_release_failed, woke, slept, idled, copied, attached | ORCH, PLUG |
| Box-agent environment | in Box: `.moor/brief.md` (0600), `.moor/reports/`, `.moor/setup.log/.rc`, `.moor/agent-state.json` + `agent-hooks.json` (Claude hooks), env `ANTHROPIC_BASE_URL`, `CLAUDE_CODE_OAUTH_TOKEN=<nonce>`, `GIT_CONFIG_*` insteadOf, `MOOR_PUSH_URL`, `gh` shim (`pr create|view|list|comment`, Task repo only, `--title`+`--body` required, `--repo/-R/--head owner:` only if matching origin or canonical name, else exit 2); claude argv `Read and follow .moor/brief.md` (+ `--dangerously-skip-permissions --settings`); detached HEAD, push by `git push "$MOOR_PUSH_URL" HEAD:refs/heads/<branch>` | BOX |
| Host-agent rules (brief) | `moor run <task> -- cmd` is the ONLY build/test path; no local tooling; real exit code, no `tail`/`head` pipes; missing tool -> stop and say; commit on current branch, no push, no branch switch, do not release Box; one simple command per Bash call (no `&&;|cd`, no `git -C`) | HOST |
| Box-agent rules (brief) | full control incl. sudo; install tools from official sources only (box-toolchain skill, mise first); commit messages follow repo hook; no Co-Authored-By; push only when guard allows (not per commit); commit-only if `$MOOR_PUSH_URL` empty; Goal-named branch beats `moor/<task>`; setup-failed line when `setup_exit` set; never `moor run` against own Box | BOX |
| Config files | user `~/.config/moor/config.yaml` (git, github.identity, credentials, drivers.crabbox.{max_boxes,ssh_port,sleep_after_minutes}, reports.backend, mcp_servers, dotfiles, box.{provider,size,enforce_egress}); repo `.moor.yaml` (tools, setup, teardown, provision, run.{env,path}, credentials, box_credentials, egress, agent.{location,model}, box.size); `box.provider` in repo = error; `record.keep` = error; `GH_TOKEN` as ref = error | HUMAN, ORCH |
| Workflows promised | (1) start -> goal -> wait -> read Reports -> steer -> close -> remove; (2) gate: `run --record ci` on Box, hook `gate check`; (3) Box agent: start `--agent-location box`, wait, `cp` report, `report --file`, host fetch + `run --record` to attest; (4) relaunch dead box agent via `start --goal`; (5) capacity queue `waiting_for_box`, promotion on next start/list/close; (6) herdr start-here/steer/close from plugin | all |

## 3. Requirements from history (rewrite MUST meet)

| REQ | Requirement | Source |
|---|---|---|
| REQ-01 | State reads (`list`, `pane`, `wait`, agent state) never take the Box lock and never block on a running `moor run`; return within the caller's bound | UL-66, 79, 81, 82, 83; 3j-b checks 2-3 (fixed there); BACKLOG "lock fixes" |
| REQ-02 | `wait --timeout N` returns within N + ssh-connect; every herdr/ssh call in a read carries the deadline (hung herdr killed) | UL-45, 55(3f), 78, 3j-b |
| REQ-03 | Only a fresh hook report may mark an agent idle; hook-idle cached within TTL (120s) stands (`hooks-cached`); older/absent cache or timed-out read is `unknown` (counts busy), never idle; never idle by guess | D36; UL-74, 82, 83; 3i FAIL |
| REQ-04 | `pane`, `wait`, `list` use ONE state-source selection (no "idle (pane)" vs "idle (hooks)" at the same instant) and one read per call (no double `agent_view`) | UL-49, 61, 66; BACKLOG list-twice |
| REQ-05 | Agent state from Claude hooks (UserPromptSubmit working, Stop idle, Notification blocked) written to a file on the Box; fall back to herdr status, then pane text restricted to last ~6 lines; "Waiting for N background" => working; "· done" => idle | UL-38, 31; D36 |
| REQ-06 | Distinguish finished from needs-input: idle Notification after Stop must not turn idle into blocked; blocked only for real prompts (permission, elicitation, AskUserQuestion, ExitPlanMode) | UL-50 (3e) |
| REQ-07 | Right after start state is `starting` (counted busy up to 120s); no false idle for an immediate waiter; hook drop on one poll must not misread pane as idle | UL-44, 49(3e), 56 |
| REQ-08 | `wait` JSON `run` field reports the in-flight run, not the last finished one | UL-95 |
| REQ-09 | `list` working->idle reconciliation happens on every read, not only on `start`; Box records idle so auto-sleep works; sleep/wake have an explicit manual path | UL-93, 94, 15(d1); BACKLOG |
| REQ-10 | Close never claims a Box deleted after a failed release: `box_deleted:false`, event `box_release_failed`, handle kept, moor-authored warning (not crabbox raw stderr), still closes Workspace/sessions | UL-87, 98 (3j-b 7 shows fixed rc but no warning); D21 |
| REQ-11 | Close deletes (not parks) the Box; retained/stopped Boxes hold provider quota | UL-46, 47; D21 |
| REQ-12 | Close tolerates already-missing worktree/Workspace/Box/sessions; a Task whose Box was never created can be closed | UL-56(d1), 87, 48 |
| REQ-13 | Failed Box acquire releases whatever the attempt created (store attempted handle) and never leaks a billing Box that holds the per-user slot | UL-20, 21; crabbox fork F2 |
| REQ-14 | close/remove separation: remove only closed Tasks, refuses unpushed/uncommitted unless `--discard`, never removes `--worktree`-bound checkouts; close never touches checkout | D21; UL-48 |
| REQ-15 | Work is not lost: Box commits are pushed or fetched; host sync re-seeding Box `.git` must not silently drop unpushed Box commits (guard needed); never `moor run` against a Box whose agent is editing in place | BACKLOG (re-seeds), SKILL rule, D9 |
| REQ-16 | Registry reconciles with the Driver (Box deleted behind moor's back -> `gone`, not `ready`/`stopped`); no raw SQL needed; `gone` Box does not count toward capacity | UL-54, 55(d1); SKILL Box |
| REQ-17 | Failures name the cause: setup_failed reason carries acquire/quota text, not generic `exit 1`; setup log records command line + exit + failing step number (empty log is a bug); tail is setup output not sync chatter; error blocks printed ONCE | UL-23, 59, 60, 88; D26 |
| REQ-18 | Credential/GitHub mint failure is its own `credential_failed` event with the GitHub error, surfaced by `env` preflight BEFORE a Box is provisioned; not reported as "dotfiles could not run" | UL-63, 64 (3g) |
| REQ-19 | Setup never strands a Box: failed `setup:` leaves Box ready, Task working, agent launched with the failure line; exit 0 + `warnings` | D26; UL-(3f e) |
| REQ-20 | Capacity: when `max_boxes` or provider quota is hit, queue as `waiting_for_box` instead of provisioning and failing; error names the slot limit/holder; prefer structured capacity exit code from Driver; promotion stops after capacity refusal | UL-22, 47, 85, 86; BACKLOG; D15 |
| REQ-21 | Dead provisioning (setting_up, no handle, empty log, >120s) detected: `start`/`run` say `provisioning dead; run moor close` instead of re-attaching forever | UL-62 |
| REQ-22 | Stdout hygiene: `--json` stdout is pure JSON; Driver/provider chatter goes to stderr or run log; `--quiet` filters by blocks (e.g. `run context:` with indented continuation) and covers `start` too; command output itself never filtered | UL-58, 59, 65, 66, 90, 4(d3), 38 |
| REQ-23 | `run` argv: one arg = shell string run under bash; several = quoted words; exit code propagates (including through pipelines, document `pipefail`); signal kill = 130 + interrupted; run recorded once (not `run_started` twice) | UL-25, 57(d2), 37 (unverified), 96, 30 |
| REQ-24 | Run cost: `--no-sync`; skip sync when tree unchanged; sync must not wipe produced files/artifacts; cp without sync for copy-back; no constant 3-4s owner wait; `moor cp` prints something on success | UL-26, 28, 31, 39, 51, 53, 44, 91, 92, 97 |
| REQ-25 | Box sync seeds `.git` from LOCAL (unpushed branches work), carries host `.git/info/exclude` and excludes `.crabbox/`; `info/` dir exists | UL-24, 29, 34, 42; fork F3 |
| REQ-26 | Gate attestation: moor-held, sha-keyed result (sha+name+command+exit), verified via `gate check`, not file copy; `--record` runs from fresh workdir so a stale Box-local record cannot satisfy it; any-pass-wins is current rule (revisit for flaky) | UL-35, 52, 72; D22; BACKLOG |
| REQ-27 | `--record` hermetic enough: root-owned files from compose containers handled (sudo chown), Docker volumes reset (repo `setup:`), deps cache implicit by lockfile+pins+setup text+uname, key promoted only after success; no `record.keep` | UL-23(3b), 43, 72, 73, D27 |
| REQ-28 | `gate check`: short sha (>=4 hex) ok, hooks use `--name` only (recorded command is the whole bootstrap string) | UL-26(3b), 61 |
| REQ-29 | Config resolution (`.moor.yaml`, setup script, RepoEnv) reads the BOUND worktree first, not the main checkout; `start --worktree` records the right base | UL-13, 64, 75; BACKLOG (RepoEnv still main) |
| REQ-30 | Box provider is per-person only (never repo); size defaults to smallest; repo `.crabbox.yaml` class must not leak into moor Boxes; `env` previews provider/size before start (decision 46: no location preview); one Task may later use several Boxes | D25, 22; UL-12, 14, 17, 33, 64 |
| REQ-31 | Credentials never in argv, env of run shells, logs, events, registry, tool output or ps; `GH_TOKEN` is not a ref; nonce never printed; Box token files removed on close; Claude token never in Box (proxy placeholder); App key never in Box | D13, 29, 32, 33; UL-20(3b), 67, 68, 70, 75, 3g/5 |
| REQ-32 | Brief delivered as a file, not argv/scrollback; no host paths or `moor run` in Box brief; Box agent reports via `.moor/reports/` only | UL-19, 20 (3b), 3c b; D-28 |
| REQ-33 | Box agent readiness: onboarding/theme/folder-trust preseeded (`~/.claude.json`), first-run prompt detection => `agent_prompt_blocked`; dotfiles (plugins, attribution blank, model) applied once per new Box only for box-agent Tasks; Box agent committed attribution rules honoured | UL-10, 76, 14(3), 15; D23, 24 |
| REQ-34 | Box agent has a checkout it can push from: remote/URL (`MOOR_PUSH_URL` from origin, canonical repo name), Task branch vs Goal-named branch rule stated, brief defers to repo hooks/guards | UL-17(3b), 47, 51, 52, 65, 80 |
| REQ-35 | Dead/exited box agent: pane stays with `moor: agent exited <code>` + resume hint; `start --goal` relaunches (decision 46: "keeps location" no longer applies); `steer` on dead pane errors with exact fix | UL-11, 12 (3), 28(3b) |
| REQ-36 | Steer: clear input line first, submit via ctrl+m (not `enter`), one verified retry, never double-submit; menu answers by number; steer to a blocked Box agent answers the prompt (decision 47; was: refused) | UL-21, 32, 48(3e) |
| REQ-37 | Transcript saved on close and relaunch (and ideally on exit), 0600, not cumulative-duplicated | UL-39, 40 (3d) |
| REQ-38 | Proxy: one supervisor per Task (no pid-0 double spawn), survives CLI exit, tunnel auto-restart with events, closes with Task, bound 127.0.0.1, nonce-gated 401, other repos/paths 403, latency overhead target < 300 ms (measured +486 ms: FAIL), canonical repo name rewrite on API route, `proxy check` explains 403 expected | D31, 33-35; UL-69, 65, 80, 75; BACKLOG |
| REQ-39 | Unattended PR from Box works: `gh` shim accepts `--repo/--head` matching Task repo; Box claude launched with bypass-permissions ONLY when proxy active (no real secret in Box, GitHub scoped to Task repo) | D35; UL-76, 77, 80 |
| REQ-40 | Plugin `close` runs moor in its own session; any in-pane moor invocation that removes its own Workspace must be detached | V09 (t09 notes); D-45 |
| REQ-41 | Herdr Workspace/worktree: bind non-herdr worktrees (`.claude/worktrees/...`) cleanly; close must not assume herdr owns a bound worktree; `start` without Workspace gives actionable error (or creates one) | UL-3, 18, 48 (d1); D11, D21 |
| REQ-42 | Egress: allow-list per repo via Driver; namespace-devbox enforce opt-in, warn when not enforced; `api.github.com`/`api.anthropic.com` dropped for Box agents; never silent fallback | D14, 34; UL-3h notes |
| REQ-43 | Compose isolation: stacks torn down by repo `teardown:` through the Box on close (failure never blocks close); per-task `COMPOSE_PROJECT_NAME` on shared hosts | D38; BACKLOG (P4) |
| REQ-44 | Skill stays discoverable from non-moor sessions ("port is already allocated", "stack busy") | UL-1 |
| REQ-45 | Box provider facts moor must not hide: per-provider sleep/wake ability (Namespace wake host-key bug; sleep disabled where wake fails), Docker state lost on stop, provider capability/readiness reporting | UL-10, 11, 15, 7, 8; BACKLOG |
| REQ-46 | Version output includes commit hash so a live binary is identifiable | UL-89 |
| REQ-47 | Tests/QA must not leak herdr Workspaces or boxes; never prune boxes a test did not create | UL-5; CLAUDE.md |
| REQ-48 | Agents cannot sleep (decision 46: all agents are Box agents): provide a blocking `wait`; waiting blocks cheaply and is bounded | UL-9(3) |
| REQ-49 | Reports never gate close or change state; `close` has no "delivered" check; the only data-loss gate is `remove` | ADR 0003, D7, D21 |
| REQ-50 | Dotfiles/Box bootstrap latency: dotfiles on every Task (decision 47); start speed (75s vs 18s) is a design item | UL-8(3), 14, e(3b) |
| REQ-51 | Quota/plan visibility: Box agents draw owner's Claude plan; surface later | UL-35(3c) (deferred) |
| REQ-52 | Agent spec states: `starting` counts as working; `unknown` counts busy; `done`/`failed`/`stopped` terminal for session | skill; D36 |

## 4. Open defects / backlog NOT to port (or to redesign deliberately)

Open at end of usage log (3j-b, moor 7d60b5f): UL-89 no commit hash in `--version`; UL-90 `--quiet start` still prints Namespace banner; UL-91/92 per-run full re-sync + constant 3.7s owner wait; UL-93 `list` shows working while idle (box never idles, never auto-sleeps); UL-94 no manual sleep/wake (`moor sleep` unrecognized); UL-95 `wait --json` run field = last finished run; UL-96 `run_started` recorded twice; UL-97 `cp` silent on success; UL-98 close of unreleased Box shows only crabbox stderr; UL-99 proxy log starts `HEAD none - 401` plus an Anthropic 429 at session start; UL-85 slot-full message omits limit; UL-86 no provider quota awareness; UL-88 start error printed 3x (partially fixed).

Verified only partially / unverified: UL-37 `run` exit 0 with failing inner pipeline; UL-84 old-origin-name rewrite on live Box shim; UL-64/75 wrong `base` on `--worktree`; UL-69/3g-6 proxy latency +486 ms; UL-3g-5 `ghp_` hits unconfirmed false positives; `record` prelude effect of `git clean` on next sync unmeasured; herdr `agent_status` for claude over ssh unverified.

Defects by design in frozen code (BACKLOG):
- RepoEnv / setup script read from the main checkout, not bound worktree.
- `run --quiet` prefix list only unit-tested; block filtering unproven live.
- `list` calls `agent_view` twice per box-pane Task (each up to 20s on hook timeout).
- `is_busy` treats invalid pids (u32::MAX cast to -1) as alive -> `wait` can block forever.
- `run_teardown` uses `?` on `append_event`; a registry write failure aborts close before Box release.
- Proxy port picker unlocked between `allocate_box_port` and `write_state`; proxy state with pid 0 holds its port forever; concurrent first start can spawn two supervisors.
- ssh Box whose host disappears stays `ready`; fork ssh purge `rm -rf dir && rm -f markers` leaves markers on dir failure.
- `moor done`-era leftovers: only latest session stopped (v0), static workRoot leftovers (v0, mostly obsolete).
- Gate rule "any pass beats newer failure" (revisit); no cache across shas; rebase voids result with no shortcut (UL-41, 49).
- Box sync re-seeds `.git`, losing unpushed Box commits; no guard.
- `--box-file` missing: Reports need host `cp` + `report --file`; recorded gate needs host fetch (D28 front-door blockers).
- Box agent invisible to `herdr agent list`; pane scrollback only last screen (UL-33, 34).
- Transcript saved only on close/relaunch, cumulative duplicate content (UL-39, 40).
- Plugin: no remove action; `act.sh` calls `moor_init` twice; Task pane polling (5s subprocess per refresh) not event driven.
- Dotfiles run on every Box start incl. unneeded cases (UL-8 d3) — decision 24 keeps groundwork hooks (latency, UL-43; self-approved stop gate UL-42/53: groundwork's problem).
- Doc drift: SKILL lists agent `state` set (working/blocked/done/failed/stopped/unknown) but Box agents expose `agent_state` working/idle/blocked/starting; `moor setup` appears in no code; "Reports" in V09 notes used `moor start` without `--goal` dispatching an agent (old bug, now goal-gated).
- Deferred, do not silently inherit: comment relay (a), pi engine (b), remote test runs (c), Slack router (d), herdr-controller parity (e), per-task narrowed tokens (f), proxy for host-run scripts and `gh` token file drop, multi-Box Tasks, local-container provider, `.mise.toml` toolchain on Box, `MOOR_DEPS_CLEANED`, `record: {cache:false}`, `pr: none|push|draft`, git submodule provisioning, per-task git author identity, Claude bg `--resume` cwd pinning, global hooks inside dispatched agents.
- Crabbox fork patches F1-F3 (host-key on wake, orphan after failed acquire, `.crabbox/` exclude), `tunnel --reverse`, namespace egress, ssh stop purge: Driver-side, not moor rewrite scope but required by REQ-13/25/38.

## 5. Decisions in motive.md that constrain the design

1. Callers: orchestrating agent + supervising human; entry = skill + herdr; no user-facing CLI (an executable for skill/herdr only).
2. In scope: parallel tasks with clear status, hands-on Box access; remote test runs deferred; PR flow from agent env, not moor.
3. Task is the unit; Issue optional; free-form name.
4. nexus is not a provider; no nexus perimeter port.
5. Box + Driver; crabbox only a Driver; no crabbox terms in interface/state.
6. SQLite registry per person: current state + append-only event log.
7. moor does not define "delivered"; Reports free-form kind; backends agentic-artifacts + local.
8. Skill actions: start (idempotent), list, steer, open, close (close-refuses-unpushed superseded by 21).
9. Guarantees: work not lost, idempotent start, Reports, shared watching; conversation resume is the Engine's job.
10. Feedback: you steer; comment relay is backlog.
11. Workspace per task; herdr owns worktree; opt-in "start task here"; herdr required behind a Surface seam.
12. Environment: repo + user layers, refs only, merged view with check/cross, reject before Box on missing ref, keep Box on setup failure.
13. Credential delivery is a Driver capability: injection preferred, stdin/deleted-file fallback; proxy allowed.
14. Egress allow-list per repo via Driver.
15. Box sleeps when idle, wakes on steer/open; `waiting_for_box` queue; no silent Driver fallback.
16. Tenant = person; never holds another person's state; shared hosts = one OS user per person (ADR 0002).
17. Agent acts as the person on GitHub/Linear; per-task narrowed tokens backlog.
18. Engines: Claude Code now, pi later.
19. Freeze v0, prototype, hard cutover reusing fitting parts.
20. herdr-controller replacement is a direction only; per-repo egress and safe credential delivery borrowed; approvers out.
21. Close and remove separate; close deletes Box, never touches checkout, no push check; remove guards unpushed, refuses bound checkouts; closed Task reopens via `start --worktree` (supersedes 8).
22. Box providers are cloud/k8s sandboxes crabbox supports (Namespace devbox pilot, smallest size); host-counted sha-keyed gate result.
23. Agent setup on Box = user dotfiles (dir or git) applied after setup; no Claude-specific projection in moor; org policy out of scope.
24. Projects Driver-agnostic: no per-repo images or Makefile toolchain installs; Box agent installs toolchains from official sources (skill recipes); dotfiles projected as-is.
25. Box provider per person, never repo config; one Task may use several Boxes (open design).
26. Setup never strands a Box: failing `setup:` records event, Box and agent stay usable.
27. Recorded-run dependency cache implicit by lockfile + pins hash; no `record.keep`.
28. Box-located agent is primary path (Slack front door): new capability lands box-first (hook state, steer, wait, transcript, Reports from `.moor/reports`); host-agent-only steps are front-door blockers.
29. GitHub creds for Boxes: moor-owned per-person GitHub App, as-you (device flow, 8h, repo-narrowed) and bot (installation token, 1h); default as-you; key never in Box; Claude phase 1 setup-token.
30. ADR 0005 (agent runs in Box; host mode supported) supersedes 0004.
31. Box-to-host tunnel behind the Driver (`crabbox tunnel --reverse`); credential-injecting host proxy is moor; no TLS interception.
32. Box agents use the person's Claude subscription token only, via host proxy; App token perms: contents, pull_requests, workflows write.
33. Identity may be as-you or bot; person's GitHub credential stays host-side behind the proxy; egress makes accidents impossible.
34. `github.identity: you|bot|auto` user-level only; auto = App-you where installed else host `gh auth token`; Box egress denies api.github.com/api.anthropic.com with proxy; Namespace egress enforced only if updatable live, else warn; Box `gh` is a curl shim.
35. Bypass-permissions for Box claude only when host proxy active; hooks still run.
36. Read-timeout state: fresh hook idle within TTL stands (`hooks-cached`); older/absent = unknown; never idle by guess.
37. Provider shift toward local/self-hosted + ssh using only stock crabbox providers; qualifies only if Boxes share host resources or hotplug; pilot before changing default.
38. Mac via crabbox stock ssh provider first; per-Task static id, ssh port, tunnel port; repo `teardown:` step run through the Box before release; local-container backlogged.
39. Rewrite: (1) black-box parity suite driving only the `moor` CLI + fake externals, passed against frozen binary; (2) design full module structure, owner approval before code; (3) reimplement; (4) hard cutover. Parity = same capabilities and state outcomes; CLI/JSON may change, skill updated at cutover; orphan branch, house-rules conventional commits; M4 and 3j-b backlog fold in; live check = fake suite + scripted smoke (host Task, Namespace Box agent, ssh-host Box agent). Rejected: progressive deepening, exact CLI/JSON parity, new crate in this repo, long-lived branch replacing src/.

Note: motive.md has no decision numbered above 39 and none numbered 23 before 22 in order; 22/23 are listed out of sequence in the file (23 appears before 22). Open question 1 (agent location) is answered by D30.
