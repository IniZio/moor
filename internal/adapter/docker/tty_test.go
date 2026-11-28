package docker

import (
	"context"
	"os/exec"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

// screen gathers what a TTY prints, so a test can wait for a piece of it.
type screen struct{ got chan string }

func follow(tty *TTY) *screen {
	s := &screen{got: make(chan string, 64)}
	go func() {
		buf := make([]byte, 4096)
		for {
			n, err := tty.Read(buf)
			if n > 0 {
				s.got <- string(buf[:n])
			}
			if err != nil {
				close(s.got)
				return
			}
		}
	}()
	return s
}

// waitFor reads until want shows, calling poke each time nothing new comes for a moment.
func (s *screen) waitFor(t *testing.T, want string, poke func()) {
	t.Helper()
	var all strings.Builder
	deadline := time.After(30 * time.Second)
	for !strings.Contains(all.String(), want) {
		select {
		case b, ok := <-s.got:
			if !ok {
				t.Fatalf("the terminal ended before %q, after %q", want, all.String())
			}
			all.WriteString(b)
		case <-time.After(250 * time.Millisecond):
			poke()
		case <-deadline:
			t.Fatalf("no %q in %q", want, all.String())
		}
	}
}

func TestATTYCarriesKeysAndSizeAndItsCloseEndsTheCommandInside(t *testing.T) {
	d, err := New(adapter(t))
	if err != nil {
		t.Fatal(err)
	}
	task := domain.TaskID("7")
	desk := d.name(task, string(Desk))
	sh(t, "docker", "run", "-d", "--name", desk, testImages.Desk)
	t.Cleanup(func() { _, _ = exec.Command("docker", "rm", "-f", desk).CombinedOutput() })
	tty, err := d.ExecTTY(context.Background(), task, Desk, 100, 30, "bash.real", "-c", `: tty-test; stty size; while read l; do echo "got:$l"; stty size; done`)
	if err != nil {
		t.Fatal(err)
	}
	s := follow(tty)
	s.waitFor(t, "30 100", func() {})
	if err := tty.Resize(120, 40); err != nil {
		t.Fatal(err)
	}
	enter := func() { _, _ = tty.Write([]byte("hi\r")) }
	enter()
	s.waitFor(t, "got:hi", enter)
	s.waitFor(t, "40 120", func() { _ = tty.Resize(120, 40); enter() })
	if err := tty.Close(); err != nil {
		t.Fatal(err)
	}
	if ps := sh(t, "docker", "exec", desk, "ps", "-eo", "args"); strings.Contains(ps, "tty-test") {
		t.Fatalf("the command outlived its terminal:\n%s", ps)
	}
}
