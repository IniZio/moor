# Task images

`make images` builds the four images a task runs on, tagged `moor-fresh-{box,desk,relay,proxy}:dev`.
`internal/adapter/docker` starts them. Builds use the host network, as the Box's own Docker
needs no proxy at build time. Base images are pinned by digest and downloaded tools by version
and checksum.

| Image | Runs as | What it is |
|---|---|---|
| box | Box | Rootless Docker in Docker with the project tools. sshd on port 2222 takes the desk's commands. |
| desk | desk | herdr-core, the terminal host, with the harness launcher and a shell wrapper. |
| relay | relay | Plain HTTP relay to the Box egress proxy, and a DNS server that refuses every query. |
| proxy | Box egress proxy, desk proxy | nono's allow-list proxy at a pinned release. |

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
- **One upstream connection per request.** nono never answers a second plain HTTP request on a
  kept-open connection. So the relay opens a new one each time.
- **The shell enters the Docker user namespace.** sshd's forced command runs `nsenter -U` into
  rootless Docker's namespace. `id -u` is 0 there, and files land as the Box user outside it.
  A nested container run with the caller's uid can write a bind mount.
- **The desk sends shell commands to the Box.** The desk's `bash` is a wrapper. With `BOX_SSH`
  set, `bash -c` runs the line in the Box over ssh.
- **Stand-in harness.** `harness` starts `standin`, which waits for `/desk/go` and runs
  three commands in the Box. A later image starts a coding agent there instead.
