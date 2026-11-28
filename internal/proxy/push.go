package proxy

import (
	"bytes"
	"compress/gzip"
	"errors"
	"fmt"
	"io"
	"net/http"
	"slices"
	"strings"

	"github.com/elazarl/goproxy"
	"github.com/go-git/go-git/v5/plumbing"
	"github.com/go-git/go-git/v5/plumbing/format/pktline"
	"github.com/go-git/go-git/v5/plumbing/protocol/packp"
	"github.com/go-git/go-git/v5/plumbing/protocol/packp/capability"
	"github.com/go-git/go-git/v5/plumbing/protocol/packp/sideband"

	"github.com/IniZio/moor/internal/domain"
)

// receivePack ends the path of a push over git's smart HTTP.
const receivePack = "/git-receive-pack"

// maxCommands caps the command section the proxy holds while it reads it. The pack after it is never held.
const maxCommands = 1 << 20

// The refusals of the push guard, which no rule can allow.
const (
	pushDefault = "pushing to the default branch is not allowed from a task"
	pushUnread  = pushDefault + ": the default branch could not be read, so main and master are refused"
	pushDelete  = "deleting a branch is not allowed from a task"
	pushTag     = "pushing tags is not allowed from a task"
	pushOther   = "another ref of this push is not allowed from a task"
	pushGarbled = "a push whose commands cannot be read is not allowed from a task"
)

var errNoFlush = errors.New("the command list has no flush")

// reportStatusV2 is the report-status git asks for when the server offers it. Its rejected lines read the same.
const reportStatusV2 = capability.Capability("report-status-v2")

// guardPush reads the commands of a push and refuses it when one of them is off limits. A push that passes goes
// on with the commands as read and the rest of its body streamed untouched.
func (p *Proxy) guardPush(r *http.Request, pol *domain.Policy, host string, port int, path string, rule domain.Rule) *http.Response {
	refuse := func(details ...string) {
		for _, d := range details {
			go p.report(host, port, domain.Method(r.Method), path, verdict{reason: domain.DenyPush, detail: d, rule: rule.Name})
		}
	}
	body, err := plainBody(r)
	if err != nil {
		refuse(pushGarbled)
		return deny(r, r.Method, host, port, verdict{reason: domain.DenyPush, detail: pushGarbled})
	}
	section, req, err := readCommands(body)
	if err != nil {
		refuse(pushGarbled)
		return deny(r, r.Method, host, port, verdict{reason: domain.DenyPush, detail: pushGarbled})
	}
	if req == nil {
		r.Body = io.NopCloser(bytes.NewReader(section))
		r.ContentLength = int64(len(section))
		return nil
	}
	reasons, details := judgePush(req.Commands, pol.DefaultBranch)
	if len(details) > 0 {
		refuse(details...)
		return pushRefused(r, req, reasons, host, port)
	}
	r.Body = struct {
		io.Reader
		io.Closer
	}{io.MultiReader(bytes.NewReader(section), body), r.Body}
	return nil
}

// plainBody is the request's body with any gzip undone. The request then goes on without the encoding.
func plainBody(r *http.Request) (io.Reader, error) {
	if r.Body == nil {
		return bytes.NewReader(nil), nil
	}
	if !strings.EqualFold(r.Header.Get("Content-Encoding"), "gzip") {
		return r.Body, nil
	}
	z, err := gzip.NewReader(r.Body)
	if err != nil {
		return nil, fmt.Errorf("gzip: %w", err)
	}
	r.Header.Del("Content-Encoding")
	r.Header.Del("Content-Length")
	r.ContentLength = -1
	return z, nil
}

// readCommands reads the pkt-lines of the command section, up to and with its flush, and decodes them with
// go-git. A lone flush with nothing after it is git's probe before a large push: it gives no request.
func readCommands(body io.Reader) ([]byte, *packp.ReferenceUpdateRequest, error) {
	var section bytes.Buffer
	s := pktline.NewScanner(io.TeeReader(io.LimitReader(body, maxCommands), &section))
	flushed, lines := false, 0
	for !flushed && s.Scan() {
		flushed, lines = len(s.Bytes()) == 0, lines+1
	}
	if !flushed {
		return nil, nil, errors.Join(errNoFlush, s.Err())
	}
	if lines == 1 {
		if n, _ := body.Read(make([]byte, 1)); n != 0 {
			return nil, nil, errors.New("a probe has nothing after its flush")
		}
		return section.Bytes(), nil, nil
	}
	req := packp.NewReferenceUpdateRequest()
	if err := req.Decode(bytes.NewReader(section.Bytes())); err != nil {
		return nil, nil, fmt.Errorf("decode the commands: %w", err)
	}
	return section.Bytes(), req, nil
}

// judgePush gives each command's refusal, empty when it may go, and the distinct refusals in order. When any
// command is refused the whole push is, so the others say so.
func judgePush(cmds []*packp.Command, defaultBranch string) ([]string, []string) {
	reasons := make([]string, len(cmds))
	var details []string
	for i, c := range cmds {
		reasons[i] = refusal(c, defaultBranch)
		if reasons[i] != "" && !slices.Contains(details, reasons[i]) {
			details = append(details, reasons[i])
		}
	}
	if len(details) > 0 {
		for i := range reasons {
			if reasons[i] == "" {
				reasons[i] = pushOther
			}
		}
	}
	return reasons, details
}

// refusal says why one command may not go, or nothing. Ref names compare without case, as some forges fold it.
func refusal(c *packp.Command, defaultBranch string) string {
	switch {
	case c.Name.IsTag():
		return pushTag
	case c.Name.IsBranch() && c.Action() == packp.Delete:
		return pushDelete
	case defaultBranch != "" && strings.EqualFold(c.Name.String(), plumbing.NewBranchReferenceName(defaultBranch).String()):
		return pushDefault
	case defaultBranch == "" && (strings.EqualFold(c.Name.String(), "refs/heads/main") || strings.EqualFold(c.Name.String(), "refs/heads/master")):
		return pushUnread
	}
	return ""
}

// pushRefused answers the way git's receive-pack does, a report-status with each ref rejected for its reason, so
// git prints the reason beside the ref. A push that asked for no report-status gets the plain 403.
func pushRefused(r *http.Request, req *packp.ReferenceUpdateRequest, reasons []string, host string, port int) *http.Response {
	if !req.Capabilities.Supports(capability.ReportStatus) && !req.Capabilities.Supports(reportStatusV2) {
		return deny(r, r.Method, host, port, verdict{reason: domain.DenyPush, detail: strings.Join(reasons, "; ")})
	}
	rs := packp.NewReportStatus()
	rs.UnpackStatus = "ok"
	for i, c := range req.Commands {
		rs.CommandStatuses = append(rs.CommandStatuses, &packp.CommandStatus{ReferenceName: c.Name, Status: reasons[i]})
	}
	var out bytes.Buffer
	var w io.Writer = &out
	band := bandOf(req.Capabilities)
	if band != 0 {
		w = sideband.NewMuxer(band, &out)
	}
	// Writes to a bytes.Buffer do not fail, and the statuses are short pkt-lines.
	_ = rs.Encode(w)
	if band != 0 {
		_ = pktline.NewEncoder(&out).Flush()
	}
	resp := goproxy.NewResponse(r, "application/x-git-receive-pack-result", http.StatusOK, out.String())
	resp.Header.Set("X-Deny-Reason", string(domain.DenyPush))
	return resp
}

// bandOf is the side-band the push asked its report in, or 0 for none.
func bandOf(caps *capability.List) sideband.Type {
	switch {
	case caps.Supports(capability.Sideband64k):
		return sideband.Sideband64k
	case caps.Supports(capability.Sideband):
		return sideband.Sideband
	}
	return 0
}
