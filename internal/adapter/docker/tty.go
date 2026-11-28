package docker

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/creack/pty"

	"github.com/IniZio/moor/internal/domain"
)

// killWait bounds the exec that ends a terminal's command inside its container.
const killWait = 5 * time.Second

// ErrNoPID is a terminal whose command did not say its process id first.
var ErrNoPID = errors.New("docker exec: the command did not start")

// TTY is a command running in one part of a task under a PTY on this host, as docker exec -it runs
// it. Docker leaves an exec running when its client goes, so Close also ends the command inside.
type TTY struct {
	pty       *os.File
	cmd       *exec.Cmd
	container string
	pid       string
	once      sync.Once
}

// ExecTTY runs argv in one part of a task under a PTY of the given size. The shell in front prints
// the process id of argv before it becomes argv, which Close needs.
func (d *Docker) ExecTTY(ctx context.Context, t domain.TaskID, p Part, cols, rows uint16, argv ...string) (*TTY, error) {
	if err := checkTask(t); err != nil {
		return nil, err
	}
	name := d.name(t, string(p))
	cmd := exec.Command("docker", append([]string{"exec", "-it", name, "sh", "-c", `echo "$$"; exec "$@"`, "sh"}, argv...)...)
	f, err := pty.StartWithSize(cmd, &pty.Winsize{Cols: cols, Rows: rows})
	if err != nil {
		return nil, fmt.Errorf("docker exec: %w", err)
	}
	tty := &TTY{pty: f, cmd: cmd, container: name}
	stop := context.AfterFunc(ctx, func() { _ = tty.Close() })
	tty.pid, err = firstLine(f)
	if _, perr := strconv.Atoi(tty.pid); err != nil || perr != nil {
		stop()
		_ = tty.Close()
		return nil, errors.Join(ErrNoPID, err)
	}
	return tty, nil
}

// firstLine reads the PTY a byte at a time up to the first newline, so nothing after it is taken.
func firstLine(f *os.File) (string, error) {
	var line []byte
	b := make([]byte, 1)
	for len(line) < 32 {
		if _, err := f.Read(b); err != nil {
			return "", fmt.Errorf("docker exec: %w", err)
		}
		if b[0] == '\n' {
			return strings.TrimSpace(string(line)), nil
		}
		line = append(line, b[0])
	}
	return "", ErrNoPID
}

func (t *TTY) Read(p []byte) (int, error) {
	return t.pty.Read(p) //nolint:wrapcheck // an io.Reader must return io.EOF itself
}

func (t *TTY) Write(p []byte) (int, error) {
	return t.pty.Write(p) //nolint:wrapcheck // an io.Writer's errors pass through as they are
}

// Resize sets the PTY's size and tells the docker client, which passes it on to the command. The
// kernel signals only a change, and a signal that came before docker listened is lost, so a repeat of
// the same size signals again.
func (t *TTY) Resize(cols, rows uint16) error {
	if err := pty.Setsize(t.pty, &pty.Winsize{Cols: cols, Rows: rows}); err != nil {
		return fmt.Errorf("docker exec: resize: %w", err)
	}
	if err := t.cmd.Process.Signal(syscall.SIGWINCH); err != nil {
		return fmt.Errorf("docker exec: resize: %w", err)
	}
	return nil
}

// Close ends the command inside the container, then the client on this host.
func (t *TTY) Close() error {
	t.once.Do(func() {
		if t.pid != "" {
			ctx, cancel := context.WithTimeout(context.Background(), killWait)
			_ = exec.CommandContext(ctx, "docker", "exec", t.container, "kill", t.pid).Run()
			cancel()
		}
		_ = t.pty.Close()
		_ = t.cmd.Process.Kill()
		_ = t.cmd.Wait()
	})
	return nil
}
