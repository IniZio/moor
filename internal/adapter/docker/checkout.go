package docker

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os/exec"
	"strings"

	"github.com/IniZio/moor/internal/domain"
)

// Checkout is what a task's Box checks out: the clone URL, the forge's host name, the ref to fetch first and the
// commit. An empty commit is the tip of the default branch.
type Checkout struct {
	URL, Host, Ref, Commit string
}

// checkoutScript checks the task's repository out in /workspace with plain git, through the Box egress proxy, which
// adds the forge credential on the wire: the Box holds none. SSH submodule URLs go over HTTPS too, so the proxy adds
// it there as well. The marker, written last, makes a repeat keep the work tree as it is and say what the first run
// checked out. A run cut short before the marker held no work, since the session starts only after it, so the next
// run starts afresh.
const checkoutScript = `set -eu
url=$1 host=$2 ref=$3 commit=$4
export http_proxy="$HTTP_PROXY" https_proxy="$HTTPS_PROXY" no_proxy="$NO_PROXY" GIT_SSL_CAINFO="$SSL_CERT_FILE" GIT_TERMINAL_PROMPT=0
cd /workspace
if [ -f .git/moor-checkout ]; then cat .git/moor-checkout; exit 0; fi
rm -rf .git
git init -q
git config url."${url%/*/*}/".insteadOf "git@$host:"
git config --add url."${url%/*/*}/".insteadOf "ssh://git@$host/"
git remote add origin "$url"
git fetch -q origin
git remote set-head origin -a >/dev/null
if [ -n "$ref" ]; then git fetch -q origin "$ref"; fi
if [ -n "$commit" ]; then
	git checkout -q -f --detach "$commit"
	what=${ref:-$commit}
	what=${what#refs/}
else
	what=$(git symbolic-ref --short refs/remotes/origin/HEAD)
	what=${what#origin/}
	git checkout -q -f -B "$what" "origin/$what"
fi
git submodule -q update --init --recursive
echo "$what at $(git rev-parse --short=12 HEAD)" > .git/moor-checkout
cat .git/moor-checkout`

// Checkout checks the repository out in the task's Box as the Box user, once: a work tree checked out before is
// kept, and Checkout gives what was checked out then, such as "main at 5f0e2b19c4d7". A failure gives git's own
// last lines, which say why.
func (d *Docker) Checkout(ctx context.Context, t domain.TaskID, c Checkout) (string, error) {
	if err := checkTask(t); err != nil {
		return "", err
	}
	var out, errOut bytes.Buffer
	cmd := exec.CommandContext(ctx, "docker", "exec", "-u", "1000", d.name(t, string(Box)),
		"sh", "-c", checkoutScript, "sh", c.URL, c.Host, c.Ref, c.Commit)
	cmd.Stdout, cmd.Stderr = &out, &errOut
	if err := cmd.Run(); err != nil {
		if msg := lastLines(errOut.String(), 5); msg != "" {
			return "", errors.New(msg)
		}
		return "", fmt.Errorf("docker exec: %w", err)
	}
	return strings.TrimSpace(out.String()), nil
}

// lastLines keeps the last n lines of text that are not blank, joined by "; " so they read as one reason.
func lastLines(text string, n int) string {
	var lines []string
	for _, l := range strings.Split(text, "\n") {
		if l = strings.TrimSpace(l); l != "" {
			lines = append(lines, l)
		}
	}
	return strings.Join(lines[max(0, len(lines)-n):], "; ")
}
