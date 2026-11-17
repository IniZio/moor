# Set up Box hosts

A Box is two Docker containers on a host. A machine's agent makes them, so a
host needs Docker and `moor-machine-agent`, and nothing else.

## Docker

Install Docker, and let the user that runs the agent use it without `sudo`.
`docker info` must work for that user. Add a teammate's machine with
[Add your machine](add-machine.md). The agent starts a Box in about 12 s on a
warm host.

The agent pulls the release's images the first time a Box needs them:
`ghcr.io/inizio/moor-agent:VERSION` and `ghcr.io/inizio/moor-work:VERSION`.
To use your own, give `up` or `run` the `--agent-image` and `--work-image`
flags, which [Box images](../images/README.md) describes.

The work place of every Box is a privileged container, because Docker runs
inside it. Run Boxes only on hosts where the user who owns them may be root.

## Prepare the Box image

The work image needs `sh`, `git`, `curl`, `tar`, and `unzip`. Build on the
release's work image, and add the tools a repo needs. `images/work` shows the
contract.

## Check the setup

```sh
docker info
moor doctor
```
