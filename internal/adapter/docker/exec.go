package docker

import (
	"context"
	"fmt"
	"io"
	"os/exec"
	"sync"

	"github.com/IniZio/moor/internal/domain"
)

// Exec runs argv in one part of a task and returns the command's stdin and stdout as one stream,
// such as the stdio of hc connect in the desk. Closing the stream ends the command, and so does ctx.
func (d *Docker) Exec(ctx context.Context, t domain.TaskID, p Part, argv ...string) (io.ReadWriteCloser, error) {
	if err := checkTask(t); err != nil {
		return nil, err
	}
	cmd := exec.CommandContext(ctx, "docker", append([]string{"exec", "-i", d.name(t, string(p))}, argv...)...)
	in, err := cmd.StdinPipe()
	if err != nil {
		return nil, fmt.Errorf("docker exec: %w", err)
	}
	out, err := cmd.StdoutPipe()
	if err != nil {
		return nil, fmt.Errorf("docker exec: %w", err)
	}
	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("docker exec: %w", err)
	}
	return &process{Reader: out, WriteCloser: in, cmd: cmd}, nil
}

type process struct {
	io.Reader
	io.WriteCloser
	cmd  *exec.Cmd
	once sync.Once
}

// Close ends the command. What it exited with does not matter to a caller that is done with it.
func (p *process) Close() error {
	p.once.Do(func() {
		_ = p.WriteCloser.Close()
		_ = p.cmd.Process.Kill()
		_ = p.cmd.Wait()
	})
	return nil
}
