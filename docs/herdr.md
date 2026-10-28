# moor in herdr

`moor-herdr` shows moor Boxes in [herdr](https://herdr.dev). Each open Box is a herdr
workspace labelled `◈ repo/name`, next to your local projects and worktrees. Its pane shows
the agent's terminal. When the Box sleeps or closes, the pane keeps a card that says what
happened. The pane goes away only when you close it.

## Install

1. Build and install the binary: `cargo install --path crates/moor-herdr`. The plugin finds it on
   PATH or in `~/.cargo/bin`; set `MOOR_HERDR_BIN` in the plugin's `env` file if it lives elsewhere.
2. Link the plugin: `herdr plugin link crates/moor-herdr/herdr`.
3. Tell it the controller socket. Put `MOOR_HERDR_SOCKET=/path/to/controller.sock` in the
   plugin's `env` file, `~/.config/herdr/plugins/config/moor/env`.
4. Add the config snippet below to `~/.config/herdr/config.toml`, then run
   `herdr server reload-config`. The plugin never edits that file.

The plugin's startup hook keeps the workspaces in step with the controller every few
seconds. `moor: show Boxes now` syncs at once.

## The config snippet

`moor-herdr herdr-setup` prints it:

```toml
# moor: set Boxes apart from local worktrees in the sidebar.
[ui.sidebar.spaces]
rows = [
  ["state_icon", { token = "workspace", rules = [{ starts_with = "◈", fg = "#c678dd", bold = true }] }, { token = "$state", fg = "#e5c07b", bold = true }],
  [{ token = "$env", dim = true, rules = [{ equals = "BOX", fg = "#c678dd", bold = true, dim = false }] }, "branch", "git_status"],
]

# moor: prefix+c runs moor's new in place of a plain tab. Merge into an existing [keys].
[keys]
new_tab = ""

[[keys.command]]
key = "prefix+c"
type = "plugin_action"
command = "moor.new"
description = "moor: new"

[[keys.command]]
key = "prefix+m"
type = "plugin_action"
command = "moor.panel"
description = "moor: Boxes"
```

Every Box workspace carries `env=BOX`, which the second row shows in colour. It also carries
`state`: empty while the Box is up, `…` while it is being set up, `◐` asleep, `◌` while it waits for a slot on the machine it claims, `✕` closed.
While the Box is up, herdr's own agent dot shows working, waiting or idle.

## New work

`prefix+c` (or `moor: new` from the command palette) opens a popup that asks `local` or `box`.

- `local` asks for a worktree name. An empty name opens a plain tab, as herdr does. A name makes
  a git worktree. A name that an open Box already has is refused, since the two would look alike
  in the sidebar.
- `box` lists the repos your open Boxes work on and takes a number or any `owner/name`. It then
  asks for a Box name, the branch or ref to start from (empty for the repo's default) and the
  first prompt for its agent. It starts the Box on the controller, shows its workspace and focuses
  it. The first prompt is optional. Leave it empty and the agent waits, so you type its first
  prompt once you attach. The Box's own branch is `moor/<name>`; give that as the base to continue
  an earlier Box's branch.

The popup closes when the work is started. A failure stays on screen until you press Enter.

## Panel

`prefix+m` (or `moor: Boxes` from the command palette) opens the Boxes panel as a split beside
the current pane. herdr's own right-click menus and `+` buttons take no plugin entries, so the
panel is where you click. Mouse capture must be on in herdr (`mouse_capture = true`, the default).

The top row has `[+ New Box]`, which opens the same popup as `prefix+c`, and `[refresh]`. Below it
each open Box is one row: its label `◈ repo/name`, its state icon and words, and its buttons.

- `[attach]` brings the Box's workspace to the front, making it first if herdr has none.
- `[wake]`, shown only on an asleep Box, wakes it.
- `[close]` closes the Box on the controller. The first click turns it into `[confirm close]`;
  a second click within four seconds closes the Box and removes its workspace from the sidebar.
  Otherwise it turns back. A Box closed anywhere else keeps its workspace, marked `✕`.

The list refreshes every 2 seconds. The line under the top row says what the last click did, or
why it failed.

| Key | Does |
| --- | --- |
| Up, Down, `k`, `j` | Select a Box. Clicking its label selects it too. |
| Enter | Attach the selected Box. |
| `w` | Wake the selected Box, when it is asleep. |
| `c` | Close the selected Box. Press it again within four seconds to confirm. |
| `n` | New Box. |
| `r` | Refresh now. |
| `q` | Close the panel. |

## Keys

| Where | Key | Does |
| --- | --- | --- |
| Attached | `ctrl+]` | Detach. The agent keeps running. |
| Card | `r` or Enter | Attach again. On an asleep Box, wake it first. On a closed Box, say it is closed. |
| Card | `q` | Close the pane. |
