package secretbox_test

import (
	"bytes"
	"errors"
	"testing"

	"github.com/IniZio/moor/internal/adapter/secretbox"
)

func key(b byte) []byte { return bytes.Repeat([]byte{b}, secretbox.KeySize) }

func mustBox(t *testing.T, k []byte) *secretbox.Box {
	t.Helper()
	b, err := secretbox.New(k)
	if err != nil {
		t.Fatal(err)
	}
	return b
}

func TestSealedValueOpensAndHidesThePlainText(t *testing.T) {
	b := mustBox(t, key(1))
	sealed, id, err := b.Seal([]byte("hunter2"))
	if err != nil || id == "" {
		t.Fatalf("seal: %q, %v", id, err)
	}
	if bytes.Contains(sealed, []byte("hunter2")) {
		t.Error("the sealed value holds the plain text")
	}
	plain, err := b.Open(sealed, id)
	if err != nil || string(plain) != "hunter2" {
		t.Errorf("open = %q, %v", plain, err)
	}
}

func TestTheSameValueSealsToDifferentBytes(t *testing.T) {
	b := mustBox(t, key(1))
	one, _, _ := b.Seal([]byte("x"))
	two, _, _ := b.Seal([]byte("x"))
	if bytes.Equal(one, two) {
		t.Error("two seals of one value are equal, so the nonce repeats")
	}
}

func TestAnotherKeyAChangedByteOrAShortValueDoesNotOpen(t *testing.T) {
	a, other := mustBox(t, key(1)), mustBox(t, key(2))
	sealed, id, _ := a.Seal([]byte("x"))
	tampered := bytes.Clone(sealed)
	tampered[len(tampered)-1] ^= 1
	for name, try := range map[string]func() ([]byte, error){
		"another key":  func() ([]byte, error) { return other.Open(sealed, id) },
		"another id":   func() ([]byte, error) { return a.Open(sealed, "zzzz") },
		"changed byte": func() ([]byte, error) { return a.Open(tampered, id) },
		"short value":  func() ([]byte, error) { return a.Open([]byte{1, 2}, id) },
		"empty value":  func() ([]byte, error) { return a.Open(nil, id) },
	} {
		if _, err := try(); !errors.Is(err, secretbox.ErrOpen) {
			t.Errorf("%s: error %v, want ErrOpen", name, err)
		}
	}
}

func TestTwoKeysHaveTwoIds(t *testing.T) {
	_, one, _ := mustBox(t, key(1)).Seal(nil)
	_, two, _ := mustBox(t, key(2)).Seal(nil)
	if one == two {
		t.Errorf("both keys have id %q", one)
	}
}

func TestAKeyOfTheWrongSizeIsRefused(t *testing.T) {
	for _, n := range []int{0, 16, 31, 33} {
		if _, err := secretbox.New(make([]byte, n)); err == nil {
			t.Errorf("a key of %d bytes was accepted", n)
		}
	}
}
