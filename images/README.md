# Task images

`make images` builds the four images a task runs on, tagged `moor-fresh-{box,desk,relay,proxy}:dev`.
`internal/adapter/docker` starts them. Builds use the host network, as the Box's own Docker
needs no proxy at build time. Base images are pinned by digest and downloaded tools by version
and checksum.

| Image | Runs as | What it is |
|---|---|---|
| box | Box | Rootless Docker in Docker with the project tools. sshd on port 2222 takes the desk's commands. |
| desk | desk | herdr-core, the terminal host, with the harness launcher, a shell wrapper and `publish`, cmd/publish built by make images: the one command that posts the agent's result page. |
| relay | relay | Plain HTTP relay to the Box egress proxy, and a DNS server that refuses every query. |
| proxy | Box egress proxy, desk proxy | cmd/proxy, built by make images: elazarl/goproxy with the task's live policy from the portal. |

## Why it is built this way

- **No `--privileged`.** The Box gets `/dev/net/tun`, `/dev/kvm` and seccomp, AppArmor and
  system paths unconfined. Rootless Docker needs no more than that. `--init` reaps the
  processes that sshd and the emulator leave behind.
- **MTU 1280 everywhere.** The task networks and the Box's inner Docker use 1280, so large
  downloads from nested containers do not stall on a smaller path MTU.
- **Two proxies on two networks.** The Box sits on an internal network with the Box egress proxy
  and the relay. The desk proxy sits on another internal network the Box is not on, so its
  address does not connect from the Box.
- **Fixed addresses in the Box network.** Containers inside the Box cannot resolve names (the
  relay refuses DNS), so the proxies sit at fixed addresses of a subnet chosen per task.
- **One upstream connection per request.** The proxy this replaced never answered a second plain HTTP request on a
  kept-open connection. So the relay opens a new one each time.
- **Tunnel by default, intercept only connection hosts.** The proxy reads its task's policy from
  the portal each second with its own token. It tunnels an allowed HTTPS host untouched, so nested
  containers that do not trust it still connect. It intercepts a host that has a connection, to add the
  value. Its certificate lands in a volume the Box or desk mounts read-only at `/ca`.
- **The shell enters the Docker user namespace.** sshd's forced command runs `nsenter -U` into
  rootless Docker's namespace. `id -u` is 0 there, and files land as the Box user outside it.
  A nested container run with the caller's uid can write a bind mount.
- **The desk sends shell commands to the Box.** The desk's `bash` is a wrapper. With `BOX_SSH`
  set, `bash -c` runs the line in the Box over ssh.
- **Stand-in harness.** `harness` starts `standin`, which waits for `/desk/go` and runs
  three commands in the Box. While it waits it answers each line typed to it with
  `standin heard: <line>`, so a terminal test can see its keys reach the harness. With a script
  in `/desk/script` it plays that instead. `publish <page> <verdict>` runs the publish tool.
  `wait <text>` reads typed lines until one holds the text. The harness exits when the script ends. A later
  image starts a coding agent there instead.
- **The publish tool.** `publish <page.html> --verdict "..."` posts the page to
  `$MOOR_PORTAL/api/tasks/$MOOR_TASK/results` as the task, with `$MOOR_TASK_TOKEN`. It goes through the desk
  proxy like every desk call. The token is in the desk only, never in the Box or a proxy.
