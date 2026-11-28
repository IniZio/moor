package wire_test

import (
	"bytes"
	"errors"
	"testing"

	"github.com/IniZio/moor/internal/wire"
)

func TestTermFramesReadBackInOrderAndBadOnesAreRefused(t *testing.T) {
	var buf bytes.Buffer
	for _, f := range []struct {
		kind wire.TermFrame
		b    []byte
	}{{wire.TermOut, []byte("hello\r\n")}, {wire.TermResize, wire.Size{Cols: 120, Rows: 40}.Bytes()}, {wire.TermExit, nil}} {
		if err := wire.WriteTerm(&buf, f.kind, f.b); err != nil {
			t.Fatal(err)
		}
	}
	kind, b, err := wire.ReadTerm(&buf)
	if err != nil || kind != wire.TermOut || string(b) != "hello\r\n" {
		t.Fatalf("first frame = %d %q %v", kind, b, err)
	}
	kind, b, err = wire.ReadTerm(&buf)
	if s, serr := wire.SizeOf(b); err != nil || kind != wire.TermResize || serr != nil || s != (wire.Size{Cols: 120, Rows: 40}) {
		t.Fatalf("second frame = %d %v %v %v", kind, s, err, serr)
	}
	if kind, b, err = wire.ReadTerm(&buf); err != nil || kind != wire.TermExit || len(b) != 0 {
		t.Fatalf("third frame = %d %q %v", kind, b, err)
	}
}

func TestBadTermFramesAreRefused(t *testing.T) {
	var buf bytes.Buffer
	if err := wire.WriteTerm(&buf, wire.TermIn, make([]byte, wire.MaxTermFrame+1)); !errors.Is(err, wire.ErrTermFrame) {
		t.Errorf("an oversized frame was written: %v", err)
	}
	if _, _, err := wire.ReadTerm(bytes.NewReader([]byte{9, 0, 0, 0, 0})); !errors.Is(err, wire.ErrTermFrame) {
		t.Errorf("an unknown kind was read: %v", err)
	}
	if _, err := wire.SizeOf(wire.Size{Cols: 0, Rows: 40}.Bytes()); !errors.Is(err, wire.ErrTermFrame) {
		t.Errorf("a zero size was read: %v", err)
	}
}
