package domain_test

import (
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

type fixedReader struct{ text domain.EventText }

func (f fixedReader) ReadEvent(domain.Event) (domain.EventText, bool) { return f.text, true }

func TestEventReadersPickTheReaderOfTheEventsSource(t *testing.T) {
	readers := domain.EventReaders{"github": fixedReader{domain.EventText{Title: "Add search"}}}
	if got, ok := readers.Read(domain.Event{Source: "github"}); !ok || got.Title != "Add search" {
		t.Errorf("github event read as %+v, %v", got, ok)
	}
	if _, ok := readers.Read(domain.Event{Source: "schedule"}); ok {
		t.Error("an event of a source with no reader was read")
	}
	if _, ok := domain.EventReaders(nil).Read(domain.Event{Source: "github"}); ok {
		t.Error("no readers read an event")
	}
}
