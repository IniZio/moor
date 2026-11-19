# moor

moor runs coding agents for your team. You start a task in the web portal, in
plain words, and the agent works on it in a Box. A Box is the computer a task
runs in, with a branch of a GitHub repo and an agent working there. You watch,
answer its questions, step in when needed, and get a pushed branch and a pull
request back. Claude Code can do the same over MCP.

The agent never holds your Claude or GitHub token. A Proxy on the host adds
each token to the agent's requests on the way out, so a Box that misbehaves
has nothing to leak.

## Where to start

- You are on a team that runs moor. Read [Get started](docs/start.md). You
  install nothing. Then read the [developer](docs/developer.md),
  [QA](docs/qa.md), [PM](docs/pm.md) or [designer](docs/designer.md) page.
- You want moor on your own Linux host. Read
  [Self-host on one machine](docs/run.md). It needs Docker and takes three
  commands.
- You want to change moor itself. Read [Development](docs/development.md).

## How the parts fit

```mermaid
flowchart LR
	P[You] -->|portal or Claude Code| G[gate]
	G --> C[controller]
	C --> N1[shared node]
	C --> N2[your machine]
	N1 --> B1[Box]
	N2 --> B2[Box]
	B1 -. token-free requests .-> X[Proxy]
	B2 -.-> X
	X -->|adds your tokens| A[GitHub, Anthropic]
```

- **Controller.** The one long-running process. It owns the state and starts,
  steers, sleeps, and closes Boxes.
- **Gate.** The HTTPS front at `https://HOST`. It signs people in with the
  team's OIDC provider, serves the portal, and serves the MCP endpoint at
  `https://HOST/mcp`.
- **Node.** A place that runs Boxes. A shared node serves everyone. A personal
  node is a teammate's own machine and serves only its owner.
- **Proxy.** It holds your secrets and adds them to requests for the hosts
  that need them, such as `api.anthropic.com` and `github.com`.

Each Box works on its own branch, `moor/ID-NAME`, and pushes there. When the
agent goes quiet, the Box sleeps. A message from any front wakes it.

## Why one piece of work per Box

A Box has its own disk, processes, and ports, so two agents never step on each
other's checkout or dev server. Closing a Box throws all of that away. Work
that was pushed stays on GitHub.

Kept caches carry over between Boxes of the same repo, so a later Box skips
most of the clone and install. [Scheduling](docs/scheduling.md) covers how that works.

## Documentation

The site at <https://inizio.github.io/moor/> has every page. Pages are grouped
by what you came to do:

- **Get started.** [Get started](docs/start.md),
  [Self-host on one machine](docs/run.md).
- **By role.** [Developers](docs/developer.md), [QA](docs/qa.md),
  [PMs](docs/pm.md), [Designers](docs/designer.md).
- **Use.** [Workflows for teams](docs/workflows.md),
  [Claude Code and the CLI](docs/boxes.md),
  [Add your machine](docs/add-machine.md), [Set up a repo](docs/repo.md),
  [Get Box events](docs/notify.md).
- **Self-host.** [Choose an install](docs/install.md),
  [Set up the Box provider](docs/providers.md),
  [Run on one host](docs/deploy.md), [Kubernetes](docs/kubernetes.md),
  [Sign-in with OIDC](docs/oidc.md), [Upgrade and back up](docs/upgrade.md).
- **Reference.** [CLI](docs/cli.md), [Configuration](docs/config.md),
  [Helm values](docs/helm.md), [MCP tools](docs/mcp.md),
  [Events](docs/events.md), [Fronts](docs/fronts.md),
  [Machine tools](docs/machines.md).
- **Concepts.** [Boxes, nodes and machines](docs/nodes.md),
  [The Proxy and secrets](docs/proxy.md), [Scheduling](docs/scheduling.md).
