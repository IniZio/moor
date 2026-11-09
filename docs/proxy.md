# The Proxy and secrets

An agent in a Box runs code it found on the internet, in a repo anyone could
have written to. So moor keeps every credential out of the Box. This page
explains how the Box still reaches GitHub and Anthropic as you.

## Secrets live with the controller

Each person links their own secrets, `claude` and `github` at least. The
controller encrypts each one with `vault.key` and stores it in its database.
Neither the database nor the key alone gives a secret back.

A start checks the secrets first. If the person lacks one that the Box's Proxy
rules need, the start is refused before anything is made. The refusal names
every missing secret at once.

## The Proxy adds them on the way out

The Proxy runs outside every Box. On one host the controller runs one shared
egress Proxy for all Boxes. On Kubernetes, `egress.enabled` runs it as replicas
of its own. Each Box reaches it through `moor-egress-forward`, which the
controller delivers into the Box, with a token that names only that Box.

Only the hosts that a Proxy rule names go through the Proxy. `tinyproxy` on the
Box's loopback splits the traffic. Requests for `api.anthropic.com` and
`github.com` go to the Proxy, and every other host, such as a package
registry, is dialled directly. The Proxy reads the secret from the vault on
each request and keeps no copy, so a changed secret applies to the next
request.

A Box sees a placeholder wherever a token would be. If the agent prints its
environment or its git config, there is nothing to leak.

## What the default rules allow

The rules are narrower than the token, so a Box cannot do everything you can.

- **Git.** A Box can fetch any repo your `github` token can read, including
  submodules. It can push only to its own repos.
- **GitHub API.** A Box can read repo metadata, releases, tags and file contents of any repo
  your token can read. For its own repos, it can also read Dependabot alerts
  and required checks. It can read the token owner's own profile, `GET /user`,
  so `gh auth status` works. Any other request on a repo outside the Box leaves
  without the token, including every write.
- **Anthropic.** The `claude` rule adds your Claude token to
  `api.anthropic.com`.

An operator can add rules under `proxy.rules`, such as a private npm registry,
and each rule names the secret it adds. A person's own MCP servers work the
same way. The Box sees `moor-proxy` in place of the key, and the Proxy sets the
real header. [Configuration](config.md#proxy) lists the rule keys.

## When the Proxy refuses

A request the rules do not allow leaves without the credential, or is refused.
moor records the refusal as an event on the Box. The event names the host and
the rule, never the token or the body. `box_status` then shows `stuck` with a kind such
as `egress_refused` or `credential_withheld`, so you can tell a blocked agent
from a slow one.

## The Slack coordinator

The coordinator in a Slack thread is a Claude Code turn on the controller
host. It uses the same idea. Its requests go through a front Proxy, which adds
the thread starter's `claude` secret. The turn itself holds no token, and it
runs in a sandbox with no route out but that Proxy.

## What the Proxy does not protect

The socket's permissions protect a Box only while it has its own network. A
Box that shares the host's network, such as a container with `network: host`,
puts its Proxy port on the host's loopback. Any process on the host could then
use that Box's credentials. Keep Boxes on their own network unless you trust
everything on the host.

A process that runs as the controller's user can read `vault.key`. The
encryption protects secrets in a copy of the database, such as a backup, and
not on a host someone else controls.
