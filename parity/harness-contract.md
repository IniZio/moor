# Parity crate shared contract (all slices MUST match these signatures exactly)

Repo: $HOME/magic/moor-next (branch `next`). Crate `parity` at $HOME/magic/moor-next/parity.
Old moor source (READ-ONLY reference): $HOME/magic/moor. Never edit or commit there. Children never commit anywhere.
Plan: .groundwork/work/rewrite/plan.md sections 1-3, 5.
Rules: edition 2021; `unsafe` only for minimal libc calls; comment density <= 5 comments per 100 lines; `cargo fmt` clean; `cargo clippy --all-targets -- -D warnings` clean for your files.
Deps (parity/Cargo.toml): serde (derive), serde_json, tempfile, rusqlite (bundled), fs2, libc, inventory 0.3, libtest-mimic 0.8.
Other slices are written IN PARALLEL by other agents against this contract; do not create or edit files outside your slice. If the crate does not build because another slice's file is missing/broken, make sure YOUR files have no errors (e.g. `cargo build -p parity 2>&1 | grep -A5 <your file>`) and report.

## Module tree (parity/src/lib.rs)
```rust
pub mod adapter;   // adapter/mod.rs declares `pub mod old;`
pub mod assert;
pub mod fake;      // fake/mod.rs declares `pub mod crabbox; pub mod herdr; pub mod ssh;` (+ private `template`)
pub mod registry;
pub mod world;     // world/mod.rs declares `pub mod config; pub mod repo;`
pub use inventory;
```

## World directory layout (root = tempdir, prefix "parity-")
```
<w>/bin/{crabbox,herdr,claude,ssh,gh}   symlinks -> the `fake` binary
<w>/home/  (.gitconfig: user.name=parity, user.email=parity@example.invalid, init.defaultBranch=main)
<w>/home/.config/  (mode 0755)   <w>/home/.config/moor/config.yaml = config_path
<w>/box/{work,home,tmp}/   <w>/state/   <w>/fakes/   <w>/tmp/   <w>/trees/   <w>/repos/
```

## fake::mod (parity/src/fake/mod.rs) -- shared by the fake binary AND the lib helpers
```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Match {
    #[serde(default, skip_serializing_if = "Option::is_none")] pub argv_prefix: Option<Vec<String>>,   // argv (excl. argv0) starts with these elements
    #[serde(default, skip_serializing_if = "Option::is_none")] pub argv_contains: Option<Vec<String>>, // each string is a SUBSTRING of some argv element
    #[serde(default, skip_serializing_if = "Option::is_none")] pub env_has: Option<Vec<String>>,       // each env var name is set
}
impl Match { pub fn matches(&self, argv: &[String], env_has: &dyn Fn(&str) -> bool) -> bool; }

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Reply {
    pub stdout: String, pub stderr: String, pub exit: i32, pub sleep_ms: u64, pub block: bool,
    pub exec: Option<String>,                    // template; run via `sh -c <rendered>` in caller cwd, HOME=<box>/home
    pub rewrite: Vec<(String, String)>,          // plain substitutions applied to the rendered exec script
    pub write_files: BTreeMap<String, String>,   // path template -> content template; parents created
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Rule {
    #[serde(default)] pub id: String,            // assigned by append/prepend when empty ("r<k>", k from <fakes>/.ruleid counter)
    #[serde(rename = "match", default)] pub matcher: Match,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub times: Option<u32>,
    #[serde(default)] pub capture_stdin: bool,
    #[serde(default)] pub reply: Reply,
}
impl Rule {   // builder; all methods consume self -> Self
    pub fn on(prefix: &[&str]) -> Rule;          // matcher.argv_prefix
    pub fn any() -> Rule;                        // matches everything
    pub fn contains(self, needle: &str) -> Self; // push to argv_contains
    pub fn env_has(self, var: &str) -> Self;
    pub fn times(self, n: u32) -> Self;
    pub fn stdout(self, s: impl Into<String>) -> Self;
    pub fn stderr(self, s: impl Into<String>) -> Self;
    pub fn exit(self, code: i32) -> Self;
    pub fn sleep_ms(self, ms: u64) -> Self;
    pub fn block(self) -> Self;
    pub fn exec(self, script: impl Into<String>) -> Self;
    pub fn rewrite(self, from: impl Into<String>, to: impl Into<String>) -> Self;
    pub fn write_file(self, path: impl Into<String>, content: impl Into<String>) -> Self;
    pub fn capture_stdin(self) -> Self;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Call {
    pub seq: u64, pub tool: String, pub argv: Vec<String>,  // argv EXCLUDES argv0
    pub cwd: String, pub pid: u32, pub ppid: u32, pub stdin: String,
    pub env: BTreeMap<String, String>,                       // prefixed vars only (readable)
    #[serde(default)] pub env_digests: BTreeMap<String, String>, // EVERY env var name -> sha256 hex of value
    #[serde(default)] pub rule: Option<String>,             // id of matched rule; None = default reply
    #[serde(default)] pub env_contains_secret: bool,        // any env VALUE contains a test secret (substring); secret never recorded
// Secrets file: World writes <fakes>/secrets (0600, one per line) and sets PARITY_SECRETS=<path>; fake reads it ($PARITY_SECRETS, else fakes_dir/secrets).
}
impl Call { pub fn flag(&self, name: &str) -> Option<String>; }  // value after `--x`, or from `--x=v`

pub struct TemplateCtx<'a> { pub argv: &'a [String], pub n: u32, pub box_root: &'a Path, pub root: &'a Path, pub cwd: &'a Path }
/// Placeholders: {argv[N]} {flag:--x} {n} {box} {root} {cwd} {last} (last argv) {post} (argv after first "--", each shell-single-quoted, space-joined). Unknown/missing -> "". Text that is not a known placeholder (e.g. JSON braces) is left verbatim.
pub fn render(template: &str, ctx: &TemplateCtx) -> String;
pub fn shell_quote(s: &str) -> String;

pub const RECORDED_ENV_PREFIXES: &[&str] = &["MOOR_", "CRABBOX_", "GH_", "GIT_CONFIG_", "ANTHROPIC_", "CLAUDE_"]; // plus exact PATH, HOME
pub fn lock(fakes_dir: &Path) -> std::io::Result<std::fs::File>;   // exclusive fs2 flock on <fakes>/.lock; unlock on drop
pub fn rules_path(fakes_dir: &Path, tool: &str) -> PathBuf;          // <fakes>/<tool>.rules.json (JSON array of Rule)
pub fn calls_path(fakes_dir: &Path, tool: &str) -> PathBuf;          // <fakes>/<tool>.jsonl
pub fn counts_path(fakes_dir: &Path, tool: &str) -> PathBuf;         // <fakes>/<tool>.counts.json (object rule id -> u32)
pub fn read_rules(fakes_dir: &Path, tool: &str) -> Vec<Rule>;        // missing file -> empty
pub fn append_rules(fakes_dir: &Path, tool: &str, rules: Vec<Rule>); // takes lock, assigns ids
pub fn prepend_rules(fakes_dir: &Path, tool: &str, rules: Vec<Rule>);// takes lock, assigns ids; prepended rules win
pub fn read_calls(fakes_dir: &Path, tool: &str) -> Vec<Call>;        // missing file -> empty
pub fn all_calls(fakes_dir: &Path) -> Vec<Call>;                     // every *.jsonl, sorted by seq
pub fn next_seq_locked(fakes_dir: &Path) -> u64;                     // caller holds lock; <fakes>/seq file, first value 1
```

## Known harness gaps
- Wrapped-nonce env: the proxy nonce is registered as a secret only after the moor call that starts the proxy, so a wrapped nonce in the env of fake calls made earlier is not flagged by `env_contains_secret`. An exact secret value in env is still caught through `env_digests`.

## Fake binary (parity/src/bin/fake.rs), uses `parity::fake::*`
1. tool = basename(argv0). fakes_dir = $PARITY_FAKES, else dirname(argv0)/../fakes (do NOT canonicalize argv0 through the symlink). root = fakes_dir.parent(); box_root = root/box.
2. Take lock. Read rules + counts. First rule whose matcher matches and whose count < times (or times None) wins; bump its count, persist counts. n = new count (0 if no rule).
3. stdin: if rule.capture_stdin read to EOF; else if stdin is not a TTY, read up to 64 KiB on a helper thread, wait at most 200 ms, take what arrived (never block on stdin).
4. seq = next_seq_locked; append one JSON line (Call) to calls_path; drop lock.
5. Reply (no rule => exit 0, no output): write_files; sleep_ms; print stdout, eprint stderr (rendered templates, verbatim, no added newline, flush); if exec: render, apply rewrite, run `sh -c <script>` with cwd = current dir, env inherited but HOME=<box>/home, inherited stdio; exit code = reply.exit if nonzero else child's code. If block: sleep forever (loop). Exit with code.
- An exec rule with `capture_stdin` records its call only after the script exits (stdin is read to EOF first, and the call line is appended after), so a blocking script is invisible to teardown's recorded pids and appears as a leak.
- A stdin pipe the caller never closes adds up to 500 ms to an exec reply (the post-exit wait for the stdin copier, `recv_timeout` at bin/fake.rs:135) and up to 200 ms otherwise (the non-exec stdin read window, bin/fake.rs:61), so callers should close stdin or use `Stdio::null`.

## fake helpers (parity/src/fake/{crabbox,herdr,ssh}.rs); each `pub fn install(w: &World)` appends defaults via w.add_rules
crabbox::install, rules in this order:
  - on(["run"]).contains("MOOR_WORKDIR") -> stdout "MOOR_WORKDIR={box}/work\n"
  - on(["run"]) -> exec "{post}"
  - on(["warmup"]) -> stdout "leased cbx_x slug={flag:--slug}\n"
  - on(["list"]) -> stdout "[]"
  - on(["stop"]) -> exit 0
  crabbox::inventory(w, names: &[&str])  // PREPEND on(["list"]) stdout = JSON array [{"name":n},..]
  crabbox::fail_quota(w)                 // PREPEND on(["warmup"]) exit 2 stderr "quota exceeded for plan\n"
  crabbox::slug(w) -> Option<String>     // --slug flag of the last recorded warmup call
herdr::install:
  - on(["worktree","list"]) -> stdout {"result":{"worktrees":[]}}
  - on(["worktree","create"]) -> exec: git -C '{flag:--cwd}' worktree add -q -b '{flag:--branch}' '{root}/trees/wt-{n}' >/dev/null 2>&1 && printf '%s' '{"result":{"workspace":{"workspace_id":"ws-{n}"},"worktree":{"path":"{root}/trees/wt-{n}","branch":"{flag:--branch}","open_workspace_id":"ws-{n}"}}}'
  - on(["pane","list"]) -> {"result":{"panes":[{"pane_id":"p-1"}]}}
  - on(["tab","list"]) -> {"result":{"tabs":[]}} ; on(["tab","create"]) -> {"result":{"root_pane":{"pane_id":"p-{n}"}}}
  - workspace create/close/rename and plugin -> default exit 0 (no rule needed)
ssh::install:
  - Rule::any() -> exec "cd '{box}' && {last}" with rewrite "/tmp/" -> "{box}/tmp/"
claude has no default rules (unmatched calls exit 0).

## world (parity/src/world/mod.rs)
```rust
pub const TOOLS: &[&str] = &["crabbox", "herdr", "claude", "ssh", "gh"];
pub struct World { /* TempDir, tracked pids Mutex<Vec<u32>>, torn_down flag */ }
impl World {
    pub fn new(fake_bin: impl AsRef<Path>) -> World;   // creates layout above, symlinks TOOLS, writes .gitconfig
    pub fn root(&self) -> &Path;  pub fn bin_dir(&self) -> PathBuf;  pub fn home(&self) -> PathBuf;
    pub fn box_root(&self) -> PathBuf;  pub fn state_dir(&self) -> PathBuf;  pub fn fakes_dir(&self) -> PathBuf;
    pub fn tmp_dir(&self) -> PathBuf;  pub fn config_path(&self) -> PathBuf;  pub fn tool_path(&self, tool: &str) -> PathBuf;
    pub fn task_name(&self, base: &str) -> String;     // "<base>-<6 lowercase hex>", random per World
    pub fn base_env(&self) -> Vec<(String, String)>;   // PATH=<w>/bin:/usr/bin:/bin HOME=<w>/home XDG_CONFIG_HOME=<home>/.config XDG_STATE_HOME=<home>/.local/state XDG_CACHE_HOME=<home>/.cache MOOR_STATE_DIR=<w>/state MOOR_CONFIG=config_path GIT_CONFIG_GLOBAL=<home>/.gitconfig GIT_CONFIG_NOSYSTEM=1 TMPDIR=<w>/tmp PARITY_FAKES=<w>/fakes PARITY_SECRETS=<w>/fakes/secrets LANG=C.UTF-8
    pub fn register_secret(&self, secret: &str);  pub fn secrets(&self) -> Vec<String>; // append to <fakes>/secrets (0600, under fake::lock); panics on empty/newline; fake sets Call.env_contains_secret if any env value contains a registered secret
    pub fn command(&self, program: impl AsRef<OsStr>) -> std::process::Command; // env_clear + base_env, stdin null, current_dir root, process_group(0)
    pub fn calls(&self, tool: &str) -> Vec<Call>;   pub fn all_calls(&self) -> Vec<Call>;
    pub fn add_rules(&self, tool: &str, rules: impl IntoIterator<Item = Rule>);
    pub fn prepend_rules(&self, tool: &str, rules: impl IntoIterator<Item = Rule>);
    pub fn track_pid(&self, pid: u32);
    pub fn teardown(&mut self) -> Result<(), String>; // idempotent. SIGTERM every recorded Call.pid + tracked pid and their pgroups (skip our own pgid, skip pid<=1); wait <=2s; scan /proc/*/cmdline for root path (skip own pid); survivors -> SIGKILL + reap, Err("leaked processes: <pid> <cmdline>; ...")
}
impl Drop for World { /* if not torn down: teardown(); on Err panic!(msg) unless std::thread::panicking() */ }
```
world::repo: `pub struct Repo { pub path: PathBuf, pub bare: PathBuf, pub url: String }`; `impl Repo { pub fn create(w: &World, name: &str) -> Repo; pub fn head(&self, w: &World) -> String; }`
  create: bare at <w>/repos/<name>.git (git init -q --bare), work repo at <w>/repos/<name> (git init -q -b main, README, one commit "init"), url = "https://github.com/acme/<name>", `git config --global url.<bare>.insteadOf <url>` (via w.command, so it lands in GIT_CONFIG_GLOBAL), remote origin=url, `git push -q origin main`. All git via w.command("git"); panic with stderr on failure.
world::config (JSON is valid YAML; serialize with serde_json):
```rust
#[derive(Debug, Default, Clone)] pub struct UserConfig { pub provider: Option<String>, pub max_boxes: Option<u32>, pub sleep_after_minutes: Option<u32> }
impl UserConfig { pub fn provider(self, p: &str) -> Self; pub fn max_boxes(self, n: u32) -> Self; pub fn to_value(&self) -> serde_json::Value; pub fn write(&self, w: &World); }
  // {"drivers":{"crabbox":{"max_boxes":..,"sleep_after_minutes":..}},"box":{"provider":..}} omitting unset keys/empty objects; writes w.config_path() (creates parent)
#[derive(Debug, Default, Clone)] pub struct RepoConfig { pub model: Option<String>, pub setup: Option<String> }
impl RepoConfig { pub fn to_value(&self) -> serde_json::Value; pub fn write(&self, repo_dir: &Path); } // {"agent":{"model"},"setup":..} -> <repo>/.moor.yaml
```

## adapter (parity/src/adapter/{mod,types,env}.rs + old/{mod,env,exec,registry,args,runs}.rs)
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum ExitClass { Ok, Usage, Refused, Failed }
#[derive(Debug, Clone)] pub struct Outcome<T> { pub exit: i32, pub class: ExitClass, pub stdout: String, pub stderr: String, pub warnings: Vec<String>, pub value: Option<T> }
impl<T> Outcome<T> { pub fn ok(&self) -> &T; } // panics showing exit/stdout/stderr unless class==Ok && value is Some
#[derive(Debug, Clone, PartialEq, Eq)] pub enum TaskState { Checking, Rejected, WaitingForBox, SettingUp, Working, SetupFailed, Idle, Asleep, Closed, Other(String) }
#[derive(Debug, Clone, PartialEq, Eq)] pub enum BoxState { Starting, Ready, Failed, Gone, Stopped, Deleted, ReleaseFailed, Asleep, Other(String) }
#[derive(Debug, Clone)] pub struct BoxView { pub state: BoxState, pub handle: Option<String> }
#[derive(Debug, Clone)] pub struct TaskView { pub name: String, pub state: TaskState, pub boxed: Option<BoxView>, pub worktree: Option<String>, pub workspace_id: Option<String>, pub raw: serde_json::Value }
#[derive(Debug, Clone)] pub struct CloseView { pub task: TaskView, pub box_deleted: bool, pub workspace_closed: bool, pub sessions_stopped: Vec<String> }
#[derive(Debug, Clone, PartialEq)] pub struct Event { pub kind: String, pub payload: serde_json::Value }
#[derive(Debug, Clone, Default)] pub struct StartOpts { pub issue: Option<String>, pub goal: Option<String>, pub worktree: Option<PathBuf>, pub model: Option<String>, pub provider: Option<String>, pub size: Option<String> }
pub trait Moor {
    fn name(&self) -> &'static str;
    fn start(&self, w: &World, cwd: &Path, name: &str, opts: &StartOpts) -> Outcome<TaskView>;
    fn list(&self, w: &World, cwd: &Path) -> Outcome<Vec<TaskView>>;
    fn close(&self, w: &World, cwd: &Path, name: &str, force: bool) -> Outcome<CloseView>;
    fn events(&self, w: &World, name: &str) -> Vec<Event>;   // events of all tasks with that name, ordered by event id
}
pub fn from_env() -> Result<Box<dyn Moor>, String>; // MOOR_BIN required and must exist (Err otherwise); MOOR_CLI "old" (default) -> OldCli; "new" -> Err("new adapter not implemented")
pub fn exit_class(code: i32) -> ExitClass; // old mapping: 0 Ok, 2 Usage, else Failed
```
Old-adapter location shim (decisions 46-47): the old binary still has an agent-location setting, defaulting to the host. `adapter/old/location.rs` pins it to the Box for the duration of `start` and `env`: a scoped `Pin` merges `agent: {location: box}` into `.moor.yaml` in the main checkout, any `opts.worktree` (pinned too), and any bound worktree (adding `.moor.yaml` to `.git/info/exclude` where needed), and restores every touched file to its prior bytes (or absence) on drop. After `start`, `settle_worktree` removes a shim-copied `.moor.yaml` from a worktree created during start, if it is untracked and holds only the shim content. Tests never mention a location. On new, `agent.location` is an unknown key and StrictConfig rejects it.

old/mod.rs: `pub struct OldCli { pub bin: PathBuf }`, `impl Moor for OldCli`, `pub fn seam_env(w: &World) -> Vec<(String, String)>`.

## registry + assert
```rust
// registry.rs
pub struct Case { pub id: &'static str, pub covers: &'static [&'static str], pub tags: &'static [&'static str], pub run: fn() }
inventory::collect!(Case);
pub fn cases() -> Vec<&'static Case>;   // sorted by id
#[macro_export] macro_rules! parity { ($id:ident, covers = [$($c:expr),* $(,)?], tags = [$($t:expr),* $(,)?], $body:block) => { fn $id() $body  $crate::inventory::submit! { $crate::registry::Case { id: stringify!($id), covers: &[$($c),*], tags: &[$($t),*], run: $id } } }; }
// assert.rs (all panic with a readable listing of the recorded tool + argv sequence)
pub fn assert_order(calls: &[Call], expected: &[(&str, &[&str])]);  // expected (tool, argv_prefix) appear as an in-order SUBSEQUENCE of calls (sorted by seq)
pub fn assert_called(calls: &[Call], tool: &str, prefix: &[&str]);
pub fn assert_never_called(calls: &[Call], tool: &str, prefix: &[&str]);
pub fn assert_seq_global(calls: &[Call]);   // seqs across all tools are exactly 1..=N
```

## tests (parity/tests/parity/main.rs; Cargo: [[test]] name = "parity", path = "tests/parity/main.rs", harness = false)
main.rs: `mod a_agent; mod a_lifecycle; mod b_driver; mod b_egress; mod b_run; mod c_box_agent; mod c_creds; mod c_github; mod c_proxy; mod coverage; mod d_reports; mod fake_protocol; mod harness;`; each `a_*`..`d_*` module is a directory with `mod.rs` plus section submodules. Production files cap at 400 lines and test files at 600 (`tools/file-cap.sh`); `pub fn world() -> parity::world::World { World::new(env!("CARGO_BIN_EXE_fake")) }`; `pub fn moor() -> Box<dyn Moor>` (from_env, panic on Err).
main(): parse libtest_mimic::Arguments; if MOOR_BIN unset or path missing -> eprintln + exit 1 (fail fast); build Trials from registry::cases(), each runs `std::panic::catch_unwind(case.run)` mapping panic payload -> Failed message; libtest_mimic::run(&args, trials).exit().

## Fake GitHub server (parity/src/github.rs + github/{wire,test_key,tests}.rs, PT8)

In-process, std-only HTTP/1.1 on 127.0.0.1:0, one thread per connection, replies `Connection: close`. `Drop` stops the listener and joins every thread, so nothing leaks.
```rust
let gh = FakeGithub::start();
gh.attach(&w);                    // w.set_env MOOR_GITHUB_API and MOOR_GITHUB_WEB; gh.env()/gh.url() give the same
gh.route("GET", "/app", vec![Reply::ok(&json!({"slug": "moor-bot"}))]);   // replies in order, last repeats
gh.route_when("POST", path, "refresh_token", replies);                    // only when the body contains the text
gh.reroute(..) / gh.reroute_when(..)                                      // replace routes with the same method, path, filter
gh.script_repo(&RepoFixture::new("acme", "widget"));                      // App, installation, installation tokens, repo, scoped token, device flow, user refresh; 403 for any other /repos/*
gh.requests() / gh.requests_to("POST", path) / gh.clear_requests()        // Vec<Request{method, path, headers (lowercase names), body}>
```
- Exact routes win over prefix routes (`/repos/*`); an unknown route answers 404 `{"message":"Not Found"}`.
- `Reply`: `ok`, `json(status, &v)`, `message(status, text)`, `oauth_error(code)` (200 with `{"error"}`), `redirect(status, location)`.
- `Request`: `header(name)`, `json()`, `bearer()`, `basic()` (decoded `id:secret`), `jwt()` (decoded header and claims of a Bearer JWT).
- `RepoFixture` fields (ids, tokens, `slug`, `client_id`, `can_push`) are public; `tokens(refresh, expires_in)` builds the token-endpoint body. Set the `script_device_flow` interval to 0 so login never sleeps.
- `TEST_KEY_PEM` is an RSA key for `moor github setup --key` (write it 0600).
- The old adapter keeps the `127.0.0.1:9` fallback for `MOOR_GITHUB_*` unless the World already carries those variables (`attach` sets them).
- Adapter ops (`parity/src/adapter/{github_ops,old_github}.rs`): `github_setup(w, cwd, &GithubSetupOpts) -> Outcome<GithubStore>`, `github_login(w, cwd)`, `github_raw(w, cwd, args)`. `GithubStore` is a snapshot of `<state>/github/` taken after the call: dir and file modes, `app`, `user`, `slug`, `pem`.
- The `gh` shim (PT9) can reuse `FakeGithub` with `script_repo`; for the Box shim point `MOOR_GH_API` at `gh.url()`.

## Capabilities

A test tagged `new_only(<Cap>)` runs as xfail unless the adapter lists `<Cap>` in `Moor::caps()` (`fn caps` in `parity/src/adapter/mod.rs:23`, default empty). The old adapter (`parity/src/adapter/old/mod.rs`) declares no caps, so every cap below xfails on old. The new adapter must list each cap it implements; an unexpected pass on old fails the run as stale.

- `BoxCredsHostOnly`: the Box agent pane receives no Box credentials; they reach setup only.
- `BoxMcp`: a Box agent launch with user `mcp_servers` delivers an MCP config the agent's claude uses; with no servers none is delivered; no MCP secret value appears in the Box tree, agent argv or env, fake recordings or registry (decision 47).
- `CloseGate`: close refuses unpushed work without `--force`.
- `CpConfirms`: a successful `cp` prints a confirmation.
- `EnvEgressText`: the env report's egress row never says it counts zero entries.
- `EnvPreflightScoped`: start ignores a broken credential unrelated to the Task's agent.
- `ExplicitSleep`: explicit `sleep` and `wake` commands exist.
- `GitIdentity`: the configured git identity applies to worktree and Box commits.
- `GoalOnAttach`: adding a goal to a running goalless Task dispatches the agent.
- `MoorWarning`: closing a Task whose Box was not released warns in moor words, with no raw Driver text.
- `ProxyBackoffClock`: the proxy backoff reads `proxy-backoff.json` (`ms`) from the World root; a supervisor that never becomes ready gives up after five attempts.
- `ProxyUpstream`: the proxy forwards to a configurable upstream: header swap for the real token, canonical-name rewrite, streaming.
- `RepoGhTokenRef`: a repo credentials `gh_token` reference is a hard config error.
- `RestartClosed`: starting a closed, unbound name creates a new Task.
- `ScopedInstallErrorMapped`: a scoped-token 403 for the target maps to an error naming the install page.
- `SetupRetry`: start after a setup failure reruns setup once.
- `SingleRead`: list and pane read the hook state file once per call.
- `StrictConfig`: an unknown config key fails parsing and names the key path.
- `TranscriptDedup`: close after a transcript was saved at agent exit writes no second archive with identical content (REQ-37).
- `TranscriptOnExit`: the transcript is saved (0600) when the next moor command observes the pane exit marker; observing again saves nothing more (REQ-37).
- `VersionHash`: `version` output identifies the commit.
- `WorktreeConfig`: a bound worktree's config drives setup.
- `WorktreeRepoConfig`: a bound worktree's config drives setup and Box credentials.
