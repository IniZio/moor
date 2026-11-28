package docker

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"net/netip"
	"strings"
)

const (
	netInner  = "inner"
	netDesk   = "desknet"
	netEgress = "egress"
)

// layout is what the parts of a task are wired with. It is read back from the task's networks, so
// a part recreated later, or by another adapter instance, gets the same addresses and passwords.
type layout struct {
	spec     Spec
	inner    netip.Prefix
	boxPass  string
	deskPass string
}

func (l layout) innerIP(host byte) string {
	a := l.inner.Addr().As4()
	a[3] = host
	return netip.AddrFrom4(a).String()
}

func (d *Docker) ensureNetworks(ctx context.Context, s Spec) (layout, error) {
	have, err := d.taskObjects(ctx, "network", s.Key.Task)
	if err != nil {
		return layout{}, err
	}
	byName := map[string]inspected{}
	for _, n := range have {
		byName[n.Name] = n
	}
	l := layout{spec: s}
	for _, role := range []string{netInner, netDesk, netEgress} {
		n, ok := byName[d.name(s.Key.Task, role)]
		if !ok {
			if n, err = d.createNetwork(ctx, s.Key, role); err != nil {
				return layout{}, err
			}
		}
		if err := l.read(role, n); err != nil {
			return layout{}, err
		}
	}
	return l, nil
}

func (l *layout) read(role string, n inspected) error {
	switch role {
	case netInner:
		l.boxPass = n.Labels[labelPass]
		if len(n.IPAM.Config) == 0 {
			return errors.New("docker: the inner network has no subnet")
		}
		p, err := netip.ParsePrefix(n.IPAM.Config[0].Subnet)
		if err != nil {
			return fmt.Errorf("docker: inner subnet: %w", err)
		}
		l.inner = p
	case netDesk:
		l.deskPass = n.Labels[labelPass]
	}
	return nil
}

func (d *Docker) createNetwork(ctx context.Context, k Key, role string) (inspected, error) {
	name := d.name(k.Task, role)
	args := append([]string{"network", "create", "-o", "com.docker.network.driver.mtu=1280"}, d.labels(k, "net-"+role)...)
	if role != netEgress {
		pass, err := password()
		if err != nil {
			return inspected{}, err
		}
		args = append(args, "--internal", "--label", labelPass+"="+pass)
	}
	var err error
	if role == netInner {
		err = d.createInner(ctx, args, name)
	} else {
		_, err = d.run(ctx, append(args, name)...)
	}
	if err != nil {
		return inspected{}, err
	}
	got, err := d.inspect(ctx, "network", []string{name})
	if err != nil {
		return inspected{}, err
	}
	return got[0], nil
}

// innerPool is where inner subnets come from. Docker's default pools are 172.17-31/16 and
// 192.168/16, and the Box's own Docker uses 10.231/16.
var innerPool = netip.MustParsePrefix("10.213.0.0/16")

// createInner takes the first free /24 of innerPool. The Box and its proxies sit at fixed addresses
// in it because containers inside the Box cannot resolve names.
func (d *Docker) createInner(ctx context.Context, args []string, name string) error {
	used, err := d.usedSubnets(ctx)
	if err != nil {
		return err
	}
	base := innerPool.Addr().As4()
	for i := range 256 {
		base[2] = byte(i)
		sub := netip.PrefixFrom(netip.AddrFrom4(base), 24)
		if overlaps(sub, used) {
			continue
		}
		dyn := netip.PrefixFrom(netip.AddrFrom4([4]byte{base[0], base[1], base[2], 128}), 25)
		_, err := d.run(ctx, append(args, "--subnet", sub.String(), "--ip-range", dyn.String(), name)...)
		if err == nil || !strings.Contains(err.Error(), "overlap") {
			return err
		}
	}
	return errors.New("docker: no free subnet left in " + innerPool.String())
}

func (d *Docker) usedSubnets(ctx context.Context) ([]netip.Prefix, error) {
	ids, err := d.lines(ctx, "network", "ls", "-q")
	if err != nil {
		return nil, err
	}
	all, err := d.inspect(ctx, "network", ids)
	var used []netip.Prefix
	for _, n := range all {
		for _, c := range n.IPAM.Config {
			if p, perr := netip.ParsePrefix(c.Subnet); perr == nil && p.Addr().Is4() {
				used = append(used, p)
			}
		}
	}
	return used, err
}

func overlaps(p netip.Prefix, used []netip.Prefix) bool {
	for _, u := range used {
		if u.Overlaps(p) {
			return true
		}
	}
	return false
}

func password() (string, error) {
	b := make([]byte, 16)
	if _, err := rand.Read(b); err != nil {
		return "", fmt.Errorf("docker: proxy password: %w", err)
	}
	return hex.EncodeToString(b), nil
}
