package docker

import (
	"errors"
	"fmt"
	"regexp"
	"strconv"

	"github.com/IniZio/moor/internal/domain"
)

// Key names one task's job on a host: the task, its run and its Box generation.
type Key struct {
	Task domain.TaskID
	Run  int64
	Gen  int64
}

// Newer reports whether k aims at a later Box generation than o, or at a later run of the same one.
func (k Key) Newer(o Key) bool {
	return k.Gen > o.Gen || (k.Gen == o.Gen && k.Run > o.Run)
}

type Images struct {
	Box, Desk, Relay, Proxy string
}

// Config is one adapter's view of the host. Every name it makes starts with Prefix, and it sees
// only objects whose product label is Prefix, so two prefixes never touch each other's tasks.
type Config struct {
	Prefix string
	Images Images
}

// Spec is everything Up needs for a task. BoxAllow are the hosts the Box egress proxy lets through,
// DeskAllow those the desk proxy lets through.
type Spec struct {
	Key       Key
	BoxAllow  []string
	DeskAllow []string
}

type Part string

const (
	BoxProxy  Part = "boxproxy"
	Relay     Part = "relay"
	Box       Part = "box"
	DeskProxy Part = "deskproxy"
	Desk      Part = "desk"
)

// parts is the start order: each part's upstream is running before the part starts.
var parts = []Part{BoxProxy, Relay, Box, DeskProxy, Desk}

type Container struct {
	Part    Part
	Name    string
	Key     Key
	State   string
	Running bool
}

// Task is what List finds on the host for one task. Key is the newest key among its containers.
type Task struct {
	Key        Key
	Containers []Container
}

var (
	ErrStale = errors.New("docker: the key is older than the task's")
	ErrNotUp = errors.New("docker: the task is not up")
)

type Docker struct {
	cfg Config
}

var (
	prefixRE = regexp.MustCompile(`^[a-z0-9][a-z0-9-]{0,23}$`)
	taskRE   = regexp.MustCompile(`^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,63}$`)
)

func New(cfg Config) (*Docker, error) {
	if !prefixRE.MatchString(cfg.Prefix) {
		return nil, fmt.Errorf("docker: prefix %q must be 1-24 of a-z 0-9 -", cfg.Prefix)
	}
	im := cfg.Images
	if im.Box == "" || im.Desk == "" || im.Relay == "" || im.Proxy == "" {
		return nil, errors.New("docker: every image must be named")
	}
	return &Docker{cfg: cfg}, nil
}

func checkSpec(s Spec) error {
	if err := checkTask(s.Key.Task); err != nil {
		return err
	}
	if len(s.BoxAllow) == 0 || len(s.DeskAllow) == 0 {
		return errors.New("docker: both proxies need at least one allowed host")
	}
	return nil
}

func checkTask(t domain.TaskID) error {
	if !taskRE.MatchString(string(t)) {
		return fmt.Errorf("docker: task id %q cannot name a container", t)
	}
	return nil
}

const (
	labelProduct = "moor.product"
	labelTask    = "moor.task"
	labelRun     = "moor.run"
	labelGen     = "moor.gen"
	labelPart    = "moor.part"
	labelPass    = "moor.proxy-pass"
)

func (d *Docker) name(t domain.TaskID, what string) string {
	return d.cfg.Prefix + "-" + string(t) + "-" + what
}

func (d *Docker) labels(k Key, part string) []string {
	return []string{
		"--label", labelProduct + "=" + d.cfg.Prefix,
		"--label", labelTask + "=" + string(k.Task),
		"--label", labelRun + "=" + strconv.FormatInt(k.Run, 10),
		"--label", labelGen + "=" + strconv.FormatInt(k.Gen, 10),
		"--label", labelPart + "=" + part,
	}
}

func (d *Docker) taskFilter(t domain.TaskID) []string {
	return []string{"--filter", "label=" + labelProduct + "=" + d.cfg.Prefix, "--filter", "label=" + labelTask + "=" + string(t)}
}

func keyOf(labels map[string]string) Key {
	run, _ := strconv.ParseInt(labels[labelRun], 10, 64)
	gen, _ := strconv.ParseInt(labels[labelGen], 10, 64)
	return Key{Task: domain.TaskID(labels[labelTask]), Run: run, Gen: gen}
}

// newest is the task's key as the host shows it, false when no container carries one.
func newest(cs []Container) (Key, bool) {
	var k Key
	for i, c := range cs {
		if i == 0 || c.Key.Newer(k) {
			k = c.Key
		}
	}
	return k, len(cs) > 0
}
