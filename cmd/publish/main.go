// Command publish is the desk's publish tool: the agent runs it with one page and one line, and the portal makes a
// new result version of the task. It is a plain command, as the portal's task-scope tools also are routes.
package main

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"

	"github.com/cenkalti/backoff/v5"
)

// maxPage is the portal's cap on a page, which the tool checks first so it does not send what is refused.
const maxPage = 8 << 20

const usage = "usage: publish <page.html> --verdict \"one line that says how it went\""

func main() { os.Exit(run(os.Args[1:], os.Getenv, os.Stdout, os.Stderr)) }

// say writes to the terminal, where a failed write has nowhere to be reported.
func say(w io.Writer, format string, args ...any) { _, _ = fmt.Fprintf(w, format, args...) }

type job struct {
	file, verdict       string
	portal, task, token string
	key                 string
}

func run(args []string, getenv func(string) string, out, errw io.Writer) int {
	j, err := parse(args, getenv)
	if err != nil {
		say(errw, "publish: %v\n", err)
		return 2
	}
	page, err := os.ReadFile(j.file)
	switch {
	case err != nil:
		say(errw, "publish: %v\n", err)
		return 1
	case len(page) == 0:
		say(errw, "publish: %s is empty\n", j.file)
		return 1
	case len(page) > maxPage:
		say(errw, "publish: %s is %d bytes, over the portal's cap of %d\n", j.file, len(page), maxPage)
		return 1
	}
	got, err := j.send(context.Background(), page)
	if err != nil {
		say(errw, "publish: %v\n", err)
		return 1
	}
	say(out, "published v%d, the task is %s: %s\n", got.Version, got.State, got.URL)
	return 0
}

// parse reads the page and the verdict in either order, which the flag package alone would not: it stops at the
// first argument that is no flag.
func parse(args []string, getenv func(string) string) (job, error) {
	var j job
	fs := flag.NewFlagSet("publish", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	fs.StringVar(&j.verdict, "verdict", "", "")
	var files []string
	for rest := args; ; {
		if err := fs.Parse(rest); err != nil {
			return j, fmt.Errorf("%w\n%s", err, usage)
		}
		if fs.NArg() == 0 {
			break
		}
		files, rest = append(files, fs.Arg(0)), fs.Args()[1:]
	}
	if len(files) != 1 {
		return j, errors.New(usage)
	}
	j.file = files[0]
	if strings.TrimSpace(j.verdict) == "" {
		return j, errors.New("--verdict is required: one line that says how it went")
	}
	j.portal, j.task, j.token = getenv("MOOR_PORTAL"), getenv("MOOR_TASK"), getenv("MOOR_TASK_TOKEN")
	for name, v := range map[string]string{"MOOR_PORTAL": j.portal, "MOOR_TASK": j.task, "MOOR_TASK_TOKEN": j.token} {
		if v == "" {
			return j, fmt.Errorf("%s is not set: this command runs on a task's desk", name)
		}
	}
	j.key = rand.Text()
	return j, nil
}

type receipt struct {
	Version int    `json:"version"`
	URL     string `json:"url"`
	State   string `json:"state"`
}

// send posts the page and repeats the call, under the same key, while the portal or the path to it fails. The key
// makes a repeat of a call that did get through the same version.
func (j job) send(ctx context.Context, page []byte) (receipt, error) {
	var body bytes.Buffer
	enc := json.NewEncoder(&body)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(map[string]string{"line": strings.TrimSpace(j.verdict), "page": string(page), "key": j.key}); err != nil {
		return receipt{}, fmt.Errorf("encode the call: %w", err)
	}
	endpoint := strings.TrimRight(j.portal, "/") + "/api/tasks/" + url.PathEscape(j.task) + "/results"
	b := backoff.NewExponentialBackOff()
	b.InitialInterval = 500 * time.Millisecond
	got, err := backoff.Retry(ctx, func() (receipt, error) { return j.once(ctx, endpoint, body.Bytes()) },
		backoff.WithBackOff(b), backoff.WithMaxTries(5), backoff.WithMaxElapsedTime(time.Minute))
	if err != nil {
		return receipt{}, fmt.Errorf("send the page: %w", err)
	}
	return got, nil
}

// permanent marks an error the tool must not repeat the call for.
func permanent(format string, args ...any) error {
	return backoff.Permanent(fmt.Errorf(format, args...)) //nolint:wrapcheck // Permanent only marks the error it is given
}

func (j job) once(ctx context.Context, endpoint string, body []byte) (receipt, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, endpoint, bytes.NewReader(body))
	if err != nil {
		return receipt{}, permanent("the call: %w", err)
	}
	req.Header.Set("Authorization", "Bearer "+j.token)
	req.Header.Set("Content-Type", "application/json")
	resp, err := (&http.Client{Timeout: 2 * time.Minute}).Do(req)
	if err != nil {
		return receipt{}, fmt.Errorf("reach the portal: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()
	text, _ := io.ReadAll(io.LimitReader(resp.Body, 4<<10))
	switch {
	case resp.StatusCode == http.StatusOK:
		var got receipt
		if err := json.Unmarshal(text, &got); err != nil {
			return receipt{}, permanent("the portal's answer is not a receipt: %w", err)
		}
		return got, nil
	case resp.StatusCode >= 500:
		return receipt{}, fmt.Errorf("the portal answered %d: %s", resp.StatusCode, strings.TrimSpace(string(text)))
	}
	return receipt{}, permanent("the portal refused the page, %d: %s", resp.StatusCode, strings.TrimSpace(string(text)))
}
