# Run the web front

`moor-web` is a separate daemon that serves a web page for the moor
controller. It listens only on `127.0.0.1` and asks for an access token.

1. Make a token file that only you can read:

   ```sh
   install -m 600 /dev/null ~/.config/moor-web-token
   head -c 24 /dev/urandom | base64 > ~/.config/moor-web-token
   ```

2. Start the front with the controller socket and the token file:

   ```sh
   moor-web --socket ~/.local/state/moor/controller.sock \
     --token-file ~/.config/moor-web-token
   ```

   `moor-web` prints the URL that it listens on, such as `http://127.0.0.1:34149/`.

3. Open the URL with the token added, as `http://127.0.0.1:34149/?token=<token>`.
   The page stores the token in a cookie, so later visits need no token in
   the URL.

To reach the page from another machine, forward the port over ssh.

## Command line

| Flag | Environment variable | Default | Meaning |
| --- | --- | --- | --- |
| `--socket PATH` | `MOOR_WEB_SOCKET` | required | The controller socket. |
| `--token-file PATH` | `MOOR_WEB_TOKEN_FILE` | required | The file that holds the access token. It must be mode 0600 and not empty. |
| `--port PORT` | none | 0, a free port | The loopback port. |
| `--state-dir DIR` | `MOOR_WEB_STATE_DIR` | `$MOOR_FRONT_STATE_DIR`, set by the controller that runs it, else `$XDG_STATE_HOME/moor-web`, else `$HOME/.local/state/moor-web` | The front's own state, including `web.db`. |
