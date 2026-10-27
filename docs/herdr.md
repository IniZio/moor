# moor in herdr

`moor-herdr` shows moor Boxes in [herdr](https://herdr.dev). Each open Box is a herdr
workspace labelled `◈ repo/name`, next to your local projects and worktrees. Its pane shows
the agent's terminal. When the Box sleeps or closes, the pane keeps a card that says what
happened. The pane goes away only when you close it.

## Install

1. Build and install the binary: `cargo install --path crates/moor-herdr`.
2. Link the plugin: `herdr plugin link crates/moor-herdr/herdr`.
3. Tell it the controller socket. Put `MOOR_HERDR_SOCKET=/path/to/controller.sock` in the
   plugin's `env` file under herdr's plugin config dir.
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
```

Every Box workspace carries `env=BOX`, which the second row shows in colour. It also carries
`state`: empty while the Box is up, `◐` asleep, `◌` waiting for a machine slot, `✕` closed.
While the Box is up, herdr's own agent dot shows working, waiting or idle.

## Keys

| Where | Key | Does |
| --- | --- | --- |
| Attached | `ctrl+]` | Detach. The agent keeps running. |
| Card | `r` or Enter | Attach again. |
| Card | `q` | Close the pane. |
