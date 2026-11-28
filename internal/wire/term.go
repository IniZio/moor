package wire

import (
	"encoding/binary"
	"errors"
	"fmt"
	"io"
)

// MaxTermFrame bounds the bytes of one term.attach frame. The runner sends at most 8 KiB of terminal
// output in one, and a browser message is cut to it.
const MaxTermFrame = 64 << 10

// ErrTermFrame is a term.attach frame that is too big or of no known kind.
var ErrTermFrame = errors.New("wire: bad terminal frame")

// TermRead asks for the session's current screen as text, with lines of scrollback above it.
type TermRead struct {
	Task  string `json:"task"`
	Lines int    `json:"lines"`
}

type TermReadReply struct {
	Text string `json:"text"`
}

// TermSend types text into the session and presses enter.
type TermSend struct {
	Task string `json:"task"`
	Text string `json:"text"`
}

// The modes of a term.attach. An observer's keys and size never reach the session.
const (
	TermWrite   = "write"
	TermObserve = "observe"
)

// WriteTerm writes one term.attach frame, a kind, a 4-byte big-endian length and b, in one Write.
func WriteTerm(w io.Writer, kind TermFrame, b []byte) error {
	if len(b) > MaxTermFrame {
		return ErrTermFrame
	}
	buf := make([]byte, 5, 5+len(b))
	buf[0] = byte(kind)
	binary.BigEndian.PutUint32(buf[1:], uint32(len(b)))
	if _, err := w.Write(append(buf, b...)); err != nil {
		return fmt.Errorf("wire: write terminal frame: %w", err)
	}
	return nil
}

// ReadTerm reads one term.attach frame.
func ReadTerm(r io.Reader) (TermFrame, []byte, error) {
	var hdr [5]byte
	if _, err := io.ReadFull(r, hdr[:]); err != nil {
		return 0, nil, fmt.Errorf("wire: read terminal frame: %w", err)
	}
	kind, n := TermFrame(hdr[0]), binary.BigEndian.Uint32(hdr[1:])
	if kind < TermIn || kind > TermExit || n > MaxTermFrame {
		return 0, nil, ErrTermFrame
	}
	b := make([]byte, n)
	if _, err := io.ReadFull(r, b); err != nil {
		return 0, nil, fmt.Errorf("wire: read terminal frame: %w", err)
	}
	return kind, b, nil
}

// Size is the body of a resize frame: columns then rows, each 2 bytes big-endian.
type Size struct{ Cols, Rows uint16 }

func (s Size) Bytes() []byte {
	return binary.BigEndian.AppendUint16(binary.BigEndian.AppendUint16(nil, s.Cols), s.Rows)
}

// SizeOf reads a resize frame's body. A zero dimension is refused.
func SizeOf(b []byte) (Size, error) {
	if len(b) != 4 {
		return Size{}, ErrTermFrame
	}
	s := Size{Cols: binary.BigEndian.Uint16(b), Rows: binary.BigEndian.Uint16(b[2:])}
	if s.Cols == 0 || s.Rows == 0 {
		return Size{}, ErrTermFrame
	}
	return s, nil
}
