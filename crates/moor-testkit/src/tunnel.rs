//! A fake crabbox whose Box is this host: `tunnel --reverse <port>:<socket>` listens on
//! `127.0.0.1:<port>` and carries each connection to the host socket, as the real tunnel does
//! inside the Box.

use std::io;
use std::path::Path;

use crate::write_executable;

/// The crabbox script; `@LOG@` becomes the file each call is logged to and `@PYTHON@` the
/// `python3` on the test's `PATH`, since moor runs crabbox with little of its environment.
const FAKE_CRABBOX: &str = r#"#!/bin/sh
echo "$*" >>'@LOG@'
case "$1" in
--version) echo 'crabbox 0.79.1-moor.6' ;;
tunnel)
  for arg; do
    case "$prev" in --reverse) forward=$arg ;; esac
    prev=$arg
  done
  [ -n "$forward" ] || exit 2
  exec '@PYTHON@' -c "$FORWARD" "$forward" ;;
*) exit 1 ;;
esac
"#;

/// The forwarder the tunnel runs: one thread per connection, each way copied until its end
/// closes, then half-closed on the other side.
const FORWARD: &str = r"
import json, socket, sys, threading
port, path = sys.argv[1].split(':', 1)
lst = socket.socket()
lst.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
lst.bind(('127.0.0.1', int(port)))
lst.listen(64)
print(json.dumps({'box': int(port), 'socket': path}), flush=True)
def pipe(a, b):
    try:
        while True:
            d = a.recv(65536)
            if not d:
                break
            b.sendall(d)
    except OSError:
        pass
    try:
        b.shutdown(socket.SHUT_WR)
    except OSError:
        pass
def carry(c):
    u = socket.socket(socket.AF_UNIX)
    try:
        u.connect(path)
    except OSError:
        c.close()
        return
    back = threading.Thread(target=pipe, args=(u, c), daemon=True)
    back.start()
    pipe(c, u)
    back.join()
    c.close()
    u.close()
while True:
    c, _ = lst.accept()
    threading.Thread(target=carry, args=(c,), daemon=True).start()
";

/// Writes the fake crabbox to `path`, logging each call's arguments to `log`.
pub fn write_fake_crabbox(path: &Path, log: &Path) -> io::Result<()> {
    let log = log.to_string_lossy();
    let python = python3()?;
    let python = python.to_string_lossy();
    if log.contains('\'') || python.contains('\'') {
        return Err(io::Error::other("a path holds a quote"));
    }
    let forward = FORWARD.replace('\'', "'\\''");
    let script = FAKE_CRABBOX
        .replace("@LOG@", &log)
        .replace("@PYTHON@", &python)
        .replacen(
            "#!/bin/sh\n",
            &format!("#!/bin/sh\nFORWARD='{forward}'\n"),
            1,
        );
    write_executable(path, script)
}

/// The first `python3` on `PATH`.
fn python3() -> io::Result<std::path::PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join("python3"))
        .find(|p| p.is_file())
        .ok_or_else(|| io::Error::other("no python3 on PATH"))
}
