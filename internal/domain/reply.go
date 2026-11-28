package domain

import (
	"errors"
	"fmt"
)

var ErrBadReply = errors.New("bad reply target")

// ReplyTarget says where a result line and link are posted. Both parts are opaque to the core,
// for example the source "github" and a pull request reference. The zero value means nowhere.
type ReplyTarget struct {
	Source string
	Ref    string
}

func (r ReplyTarget) Set() bool { return r != ReplyTarget{} }

// Check refuses a target with only one of its parts.
func (r ReplyTarget) Check() error {
	if r.Set() && (r.Source == "" || r.Ref == "") {
		return fmt.Errorf("%w: source %q with ref %q", ErrBadReply, r.Source, r.Ref)
	}
	return nil
}
