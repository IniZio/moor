package docker

import (
	"context"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

// seedSource makes a repository inside the Box to check out from, so the checkout needs no network.
const seedSource = `set -e; rm -rf /tmp/src.git /tmp/seed; git init -q --bare -b main /tmp/src.git; git init -q -b main /tmp/seed
cd /tmp/seed; echo one > app.txt; git add app.txt; git -c user.email=seed@example.org -c user.name=seed commit -qm one
git push -q /tmp/src.git main; git rev-parse --short=12 HEAD`

var source = Checkout{URL: "file:///tmp/src.git", Host: "forge.example"}

// checkoutOnce checks the repository out, then shows a second checkout keeps work in progress and says the same.
func checkoutOnce(t *testing.T, d *Docker, task domain.TaskID) string {
	box := d.name(task, string(Box))
	head := sh(t, "docker", "exec", "-u", "1000", box, "sh", "-c", seedSource)
	got, err := d.Checkout(context.Background(), task, source)
	if err != nil || got != "main at "+head {
		t.Fatalf("checkout gave %q, %v, want main at %s", got, err, head)
	}
	if owner := sh(t, "docker", "exec", box, "stat", "-c", "%u", "/workspace/app.txt"); owner != "1000" {
		t.Fatalf("the checked-out file is owned by %s, want the Box user 1000", owner)
	}
	sh(t, "docker", "exec", "-u", "1000", box, "sh", "-c", "echo work >> /workspace/app.txt")
	checkoutKept(t, d, task, got)
	return got
}

// checkoutKept shows a repeated checkout changes nothing and gives what the first one checked out.
func checkoutKept(t *testing.T, d *Docker, task domain.TaskID, first string) {
	again, err := d.Checkout(context.Background(), task, source)
	if err != nil || again != first {
		t.Fatalf("a repeated checkout gave %q, %v, want %q", again, err, first)
	}
	if work := sh(t, "docker", "exec", d.name(task, string(Box)), "cat", "/workspace/app.txt"); work != "one\nwork" {
		t.Fatalf("the work tree after a repeated checkout holds %q", work)
	}
}
