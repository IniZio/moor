package docker

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"

	"github.com/IniZio/moor/internal/domain"
)

func (d *Docker) run(ctx context.Context, args ...string) (string, error) {
	var out, errOut bytes.Buffer
	cmd := exec.CommandContext(ctx, "docker", args...)
	cmd.Stdout, cmd.Stderr = &out, &errOut
	if err := cmd.Run(); err != nil {
		return out.String(), fmt.Errorf("docker %s: %w: %s", strings.Join(args[:min(2, len(args))], " "), err, strings.TrimSpace(errOut.String()))
	}
	return out.String(), nil
}

func (d *Docker) lines(ctx context.Context, args ...string) ([]string, error) {
	out, err := d.run(ctx, args...)
	return strings.Fields(out), err
}

type inspected struct {
	Name   string
	Labels map[string]string
	Config struct {
		Labels map[string]string
		Env    []string
	}
	State struct {
		Status  string
		Running bool
	}
	IPAM struct {
		Config []struct{ Subnet string }
	}
	NetworkSettings struct {
		Networks map[string]struct{ IPAddress string }
	}
}

func (d *Docker) inspect(ctx context.Context, kind string, names []string) ([]inspected, error) {
	if len(names) == 0 {
		return nil, nil
	}
	out, err := d.run(ctx, append([]string{kind, "inspect"}, names...)...)
	if err != nil {
		return nil, err
	}
	var got []inspected
	if err := json.Unmarshal([]byte(out), &got); err != nil {
		return nil, fmt.Errorf("docker %s inspect: %w", kind, err)
	}
	return got, nil
}

func (d *Docker) containers(ctx context.Context, filters ...string) ([]Container, error) {
	ids, err := d.lines(ctx, append([]string{"ps", "-a", "-q", "--filter", "label=" + labelProduct + "=" + d.cfg.Prefix}, filters...)...)
	if err != nil {
		return nil, err
	}
	got, err := d.inspect(ctx, "container", ids)
	cs := make([]Container, 0, len(got))
	for _, c := range got {
		cs = append(cs, Container{
			Part: Part(c.Config.Labels[labelPart]), Name: strings.TrimPrefix(c.Name, "/"),
			Key: keyOf(c.Config.Labels), State: c.State.Status, Running: c.State.Running,
		})
	}
	return cs, err
}

func (d *Docker) taskContainers(ctx context.Context, t domain.TaskID) ([]Container, error) {
	return d.containers(ctx, "--filter", "label="+labelTask+"="+string(t))
}

func (d *Docker) taskObjects(ctx context.Context, kind string, t domain.TaskID) ([]inspected, error) {
	names, err := d.lines(ctx, append([]string{kind, "ls", "-q"}, d.taskFilter(t)...)...)
	if err != nil {
		return nil, err
	}
	return d.inspect(ctx, kind, names)
}
