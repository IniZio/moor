# moor in herdr

`moor-herdr` shows moor Boxes in [herdr](https://herdr.dev). Each open Box is a herdr
workspace labelled `◈ repo/name`, next to your local projects and worktrees. Its pane shows
the agent's terminal. When the Box sleeps or closes, the pane keeps a card that says what
happened. The pane goes away only when you close it.

## Install

1. Install the plugin. It downloads the matching `moor-herdr` release for Linux or macOS
   (x86_64 or arm64) and checks it against its SHA-256:

   ```sh
   herdr plugin install IniZio/moor/crates/moor-herdr/herdr --ref moor-herdr-v0.2.0
   ```

2. Tell it the controller socket:

   ```sh
   echo MOOR_HERDR_SOCKET=/path/to/controller.sock >> "$(herdr plugin config-dir moor)/env"
   ```

3. Add the config snippet the install wrote, then reload. The plugin never edits your config.

   ```sh
   cat "$(herdr plugin config-dir moor)/herdr-setup.toml" >> ~/.config/herdr/config.toml
   herdr server reload-config
   ```

herdr runs plugin startup hooks only when its server starts, so the Boxes follow the controller
from the next server start. Until then, `moor: show Boxes now` syncs once.

`herdr plugin update moor` fetches the same ref again. To move to a new release, install again
with its tag.

The binary and the default shell go to the plugin's state dir,
`~/.local/state/herdr/plugins/moor`, which keeps its path across updates.

To work on the plugin itself, build the binary (`cargo install --path crates/moor-herdr`) and
link the checkout with `herdr plugin link crates/moor-herdr/herdr`. A linked plugin runs the
installed release if there is one, else `moor-herdr` from PATH or `~/.cargo/bin`, and
`moor-herdr herdr-setup` prints its snippet. `MOOR_HERDR_BIN` in the plugin's `env` file
overrides the binary either way.

## Releases

Pushing a tag `moor-herdr-v<version>`, where the version is `crates/moor-herdr`'s, runs
`.github/workflows/release-herdr.yaml`. It builds static Linux binaries and macOS binaries,
checks that the plugin installs from each build, and publishes them with their checksums.
Bump the crate's version before you tag.

The plugin's startup hook keeps the workspaces in step with the controller every few
seconds. `moor: show Boxes now` syncs at once.

## The config snippet

The installed plugin writes it to `herdr-setup.toml`; `moor-herdr herdr-setup` prints it:

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

# moor: a new tab in a Box's workspace opens a shell in the Box; other tabs get your shell.
# Had you set default_shell, put it in the plugin env file as MOOR_HERDR_HOST_SHELL=<path>.
[terminal]
default_shell = "/path/to/moor/crates/moor-herdr/herdr/bin/shell.sh"
```

An installed plugin names the `shell.sh` in its state dir; `moor-herdr herdr-setup` names the checkout's `bin/shell.sh`.

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

## Shells in a Box

With the snippet's `default_shell`, every new tab or split herdr opens runs the plugin's
`bin/shell.sh`. In a Box's workspace it opens a login shell in the Box, in its work tree, over
the same relay the agent's pane uses. `exit` or `ctrl+]` ends it and closes the pane.

Every other pane gets your own shell: `MOOR_HERDR_HOST_SHELL` from the plugin's `env` file,
else `$SHELL`, else `/bin/sh`. The Box's own agent pane stays a host shell too, since sync runs
the attach in it. When the Box is asleep, still being set up, closed or unreachable, or no
controller is running, the pane says so on one line and gives you a local shell instead.

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
