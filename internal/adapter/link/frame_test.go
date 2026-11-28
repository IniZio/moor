package link

import (
	"bytes"
	"errors"
	"io"
	"strings"
	"testing"
)

func TestFrameLeavesTrailingBytes(t *testing.T) {
	var buf bytes.Buffer
	if err := WriteFrame(&buf, Request{Op: "a.b"}); err != nil {
		t.Fatal(err)
	}
	buf.WriteString("raw bytes follow")
	var rq Request
	if err := ReadFrame(&buf, &rq, true); err != nil || rq.Op != "a.b" {
		t.Fatalf("%v %v", rq, err)
	}
	if buf.String() != "raw bytes follow" {
		t.Fatalf("ReadFrame consumed raw bytes: %q", buf.String())
	}
}

func TestFrameStrictForRequestsLenientForReplies(t *testing.T) {
	frame := append([]byte{0, 0, 0, 16}, `{"op":"a","x":1}`...)
	var rq Request
	if err := ReadFrame(bytes.NewReader(frame), &rq, true); err == nil {
		t.Fatal("a request with an unknown field must be refused")
	}
	var lenient Request
	if err := ReadFrame(bytes.NewReader(frame), &lenient, false); err != nil || lenient.Op != "a" {
		t.Fatalf("a reply with an unknown field must be accepted: %v %v", lenient, err)
	}
}

func TestFrameBounds(t *testing.T) {
	var rq Request
	if err := ReadFrame(bytes.NewReader([]byte{0xff, 0, 0, 0}), &rq, true); !errors.Is(err, errFrameTooBig) {
		t.Fatalf("oversize read: %v", err)
	}
	big := Reply{Body: []byte(`"` + strings.Repeat("x", maxFrame) + `"`)}
	if err := WriteFrame(io.Discard, big); !errors.Is(err, errFrameTooBig) {
		t.Fatalf("oversize write: %v", err)
	}
	if err := ReadFrame(bytes.NewReader([]byte{0, 0, 0, 9, '{'}), &rq, true); err == nil {
		t.Fatal("a truncated frame must fail")
	}
	if err := ReadFrame(bytes.NewReader([]byte{0, 0, 0, 3, 'n', 'o', 'p'}), &rq, true); err == nil {
		t.Fatal("a frame that is not JSON must fail")
	}
}
