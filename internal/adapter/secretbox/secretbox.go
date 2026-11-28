package secretbox

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
)

// KeySize is the key length in bytes.
const KeySize = 32

var ErrOpen = errors.New("cannot open the sealed value")

// Box seals and opens values under one key. Its key id is the start of the key's SHA-256, so a stored value
// says which key sealed it without giving the key away.
type Box struct {
	aead cipher.AEAD
	id   string
}

func New(key []byte) (*Box, error) {
	if len(key) != KeySize {
		return nil, fmt.Errorf("secretbox: the key is %d bytes, want %d", len(key), KeySize)
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, fmt.Errorf("secretbox: %w", err)
	}
	aead, err := cipher.NewGCM(block)
	if err != nil {
		return nil, fmt.Errorf("secretbox: %w", err)
	}
	sum := sha256.Sum256(key)
	return &Box{aead: aead, id: hex.EncodeToString(sum[:4])}, nil
}

// Seal returns a fresh random nonce followed by the encrypted value, and the key id.
func (b *Box) Seal(plain []byte) ([]byte, string, error) {
	nonce := make([]byte, b.aead.NonceSize())
	if _, err := rand.Read(nonce); err != nil {
		return nil, "", fmt.Errorf("secretbox: nonce: %w", err)
	}
	return b.aead.Seal(nonce, nonce, plain, nil), b.id, nil
}

// Open returns the value Seal made, and ErrOpen for another key, another key id or a changed byte.
func (b *Box) Open(sealed []byte, keyID string) ([]byte, error) {
	n := b.aead.NonceSize()
	if keyID != b.id || len(sealed) < n {
		return nil, ErrOpen
	}
	plain, err := b.aead.Open(nil, sealed[:n], sealed[n:], nil)
	if err != nil {
		return nil, ErrOpen
	}
	return plain, nil
}
