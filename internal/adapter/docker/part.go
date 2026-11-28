package docker

import (
	"context"
	"fmt"
	"slices"
	"time"
)

const (
	volWorkspace = "workspace"
	volDesk      = "desk"
	volKeys      = "keys"
	volBoxDocker = "boxdocker"
	volBoxCache  = "boxcache"
	volBoxCA     = "boxca"
	volDeskCA    = "deskca"
)

var (
	taskVolumes = []string{volWorkspace, volDesk, volKeys, volBoxCA, volDeskCA}
	boxVolumes  = []string{volBoxDocker, volBoxCache}
)

type join struct{ net, ip string }

type partPlan struct {
	args  []string
	image string
	join  []join
}

func (d *Docker) plan(l layout, p Part) partPlan {
	n := func(what string) string { return d.name(l.spec.Key.Task, what) }
	im := d.cfg.Images
	switch p {
	case BoxProxy:
		return partPlan{args: d.proxyArgs(l, l.boxPass, l.spec.BoxToken, volBoxCA), image: im.Proxy, join: []join{{n(netInner), l.innerIP(2)}}}
	case Relay:
		return partPlan{args: []string{
			"--network", n(netInner), "--ip", l.innerIP(3),
			"-e", "UP=" + l.innerIP(2) + ":8080", "-e", "UP_PASS=" + l.boxPass,
		}, image: im.Relay}
	case Box:
		return partPlan{args: d.boxArgs(l), image: im.Box}
	case DeskProxy:
		return partPlan{args: d.proxyArgs(l, l.deskPass, l.spec.DeskToken, volDeskCA), image: im.Proxy, join: []join{{net: n(netDesk)}}}
	case Desk:
		return partPlan{args: d.deskArgs(l), image: im.Desk, join: []join{{net: n(netInner)}}}
	}
	return partPlan{}
}

// proxyArgs start a proxy on the egress network. It writes the certificate its clients trust into the
// volume ca, which its clients mount read-only. The token never reaches the Box or the desk.
func (d *Docker) proxyArgs(l layout, pass, token, ca string) []string {
	return []string{
		"--network", d.name(l.spec.Key.Task, netEgress), "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
		"-e", "MOOR_PROXY_PASS=" + pass, "-e", "MOOR_PORTAL=" + l.spec.Portal, "-e", "MOOR_TASK=" + string(l.spec.Key.Task),
		"-e", "MOOR_TOKEN=" + token, "-v", d.name(l.spec.Key.Task, ca) + ":/ca",
	}
}

// trustArgs mount the proxy's certificate read-only and point the usual tools at it.
func (d *Docker) trustArgs(l layout, ca string) []string {
	return []string{
		"-v", d.name(l.spec.Key.Task, ca) + ":/ca:ro", "-e", "SSL_CERT_FILE=/ca/bundle.pem",
		"-e", "NODE_EXTRA_CA_CERTS=/ca/ca.crt", "-e", "REQUESTS_CA_BUNDLE=/ca/bundle.pem",
	}
}

func (d *Docker) boxArgs(l layout) []string {
	n := func(what string) string { return d.name(l.spec.Key.Task, what) }
	return append(d.trustArgs(l, volBoxCA),
		"--hostname", n(string(Box)), "--network", n(netInner), "--ip", l.innerIP(10), "--init",
		"--device", "/dev/kvm", "--device", "/dev/net/tun",
		"--security-opt", "seccomp=unconfined", "--security-opt", "apparmor=unconfined", "--security-opt", "systempaths=unconfined",
		"-e", "DOCKER_TLS_CERTDIR=", "-e", "BOX_DNS="+l.innerIP(3),
		"-e", "HTTPS_PROXY=http://x:"+l.boxPass+"@"+l.innerIP(2)+":8080",
		"-e", "HTTP_PROXY=http://"+l.innerIP(3)+":8081", "-e", "NO_PROXY=localhost,127.0.0.1",
		"-v", n(volWorkspace)+":/workspace", "-v", n(volKeys)+":/keys:ro",
		"-v", n(volBoxDocker)+":/home/rootless/.local/share/docker", "-v", n(volBoxCache)+":/home/rootless/.cache",
	)
}

func (d *Docker) deskArgs(l layout) []string {
	n := func(what string) string { return d.name(l.spec.Key.Task, what) }
	proxy := "http://x:" + l.deskPass + "@" + n(string(DeskProxy)) + ":8080"
	return append(d.trustArgs(l, volDeskCA),
		"--hostname", n(string(Desk)), "--network", n(netDesk),
		"--cap-drop", "ALL", "--security-opt", "no-new-privileges",
		"-e", "HTTPS_PROXY="+proxy, "-e", "HTTP_PROXY="+proxy,
		"-e", "NO_PROXY="+n(string(Box))+","+n(string(DeskProxy))+",localhost,127.0.0.1",
		"-e", "HARNESS_BOX_SSH=rootless@"+n(string(Box)),
		"-v", n(volWorkspace)+":/workspace", "-v", n(volDesk)+":/desk", "-v", n(volKeys)+":/keys:ro",
	)
}

func probe(p Part) []string {
	switch p {
	case BoxProxy, DeskProxy:
		return []string{"nc", "-z", "127.0.0.1", "8080"}
	case Relay:
		return []string{"nc", "-z", "127.0.0.1", "8081"}
	case Box:
		return []string{"docker", "--host", "unix:///run/user/1000/docker.sock", "version"}
	case Desk:
		return []string{"hc", "server", "status"}
	}
	return nil
}

func (d *Docker) create(ctx context.Context, l layout, p Part) error {
	pl := d.plan(l, p)
	name := d.name(l.spec.Key.Task, string(p))
	args := append([]string{"create", "--name", name}, d.labels(l.spec.Key, string(p))...)
	args = append(append(args, pl.args...), pl.image)
	if _, err := d.run(ctx, args...); err != nil {
		return err
	}
	for _, j := range pl.join {
		a := []string{"network", "connect"}
		if j.ip != "" {
			a = append(a, "--ip", j.ip)
		}
		if _, err := d.run(ctx, append(a, j.net, name)...); err != nil {
			return err
		}
	}
	_, err := d.run(ctx, "start", name)
	return err
}

// ensureVolumes creates the named volumes of the task that do not exist yet.
func (d *Docker) ensureVolumes(ctx context.Context, k Key, want []string) error {
	for _, v := range want {
		if _, err := d.run(ctx, append(append([]string{"volume", "create"}, d.labels(k, v)...), d.name(k.Task, v))...); err != nil {
			return err
		}
	}
	return nil
}

// dropBoxVolumes removes Box volumes whose generation label names another Box than k's.
func (d *Docker) dropBoxVolumes(ctx context.Context, k Key) error {
	have, err := d.taskObjects(ctx, "volume", k.Task)
	if err != nil {
		return err
	}
	var old []string
	for _, v := range have {
		if slices.Contains(boxVolumes, v.Labels[labelPart]) && keyOf(v.Labels).Gen != k.Gen {
			old = append(old, v.Name)
		}
	}
	return d.remove(ctx, "volume", old)
}

// remove deletes the named objects of one kind, doing nothing for none.
func (d *Docker) remove(ctx context.Context, kind string, names []string) error {
	if len(names) == 0 {
		return nil
	}
	cmd := []string{kind, "rm"}
	if kind == "container" {
		cmd = append(cmd, "-f")
	}
	_, err := d.run(ctx, append(cmd, names...)...)
	return err
}

// initScript makes the key the desk signs in to the Box with. The key lands under its final name
// last, so a run cut short leaves no half key behind.
const initScript = `[ -f /keys/id ] || { rm -f /keys/new /keys/new.pub && ssh-keygen -q -t ed25519 -N "" -f /keys/new ` +
	`&& mv /keys/new.pub /keys/id.pub && mv /keys/new /keys/id; } && chown -R 1000:1000 /keys && chown 1000:1000 /workspace /desk ` +
	`&& chown 65534:65534 /boxca /deskca`

func (d *Docker) initVolumes(ctx context.Context, l layout) error {
	k := l.spec.Key
	name := d.name(k.Task, "init")
	if _, err := d.run(ctx, "rm", "-f", name); err != nil {
		return err
	}
	args := append([]string{"run", "--rm", "--name", name}, d.labels(k, "init")...)
	args = append(args, "--network", "none", "--user", "root", "--entrypoint", "sh",
		"-v", d.name(k.Task, volKeys)+":/keys", "-v", d.name(k.Task, volWorkspace)+":/workspace",
		"-v", d.name(k.Task, volDesk)+":/desk", "-v", d.name(k.Task, volBoxCA)+":/boxca", "-v", d.name(k.Task, volDeskCA)+":/deskca",
		d.cfg.Images.Box, "-c", initScript)
	_, err := d.run(ctx, args...)
	return err
}

const readyTimeout = 5 * time.Minute

func (d *Docker) waitReady(ctx context.Context, l layout) error {
	ctx, cancel := context.WithTimeout(ctx, readyTimeout)
	defer cancel()
	for _, p := range parts {
		if err := d.waitPart(ctx, d.name(l.spec.Key.Task, string(p)), probe(p)); err != nil {
			return err
		}
	}
	return nil
}

func (d *Docker) waitPart(ctx context.Context, name string, cmd []string) error {
	for {
		_, err := d.run(ctx, append([]string{"exec", name}, cmd...)...)
		if err == nil {
			return nil
		}
		select {
		case <-ctx.Done():
			lctx, cancel := context.WithTimeout(context.WithoutCancel(ctx), 5*time.Second)
			defer cancel()
			logs, _ := d.run(lctx, "logs", "--tail", "20", name)
			return fmt.Errorf("docker: %s not ready: %w; last probe: %w; logs: %s", name, ctx.Err(), err, logs)
		case <-time.After(250 * time.Millisecond):
		}
	}
}
