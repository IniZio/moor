package settings

import (
	"context"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

// Store is the four calls the service makes on the stored versions and secrets.
type Store interface {
	LatestSettings(ctx context.Context) (domain.SettingsVersion, error)
	SettingsVersions(ctx context.Context) ([]domain.SettingsVersion, error)
	SaveSettings(ctx context.Context, w domain.SettingsWrite) (domain.SettingsVersion, error)
}

// Parsed is a document that passed the parser. Text is the document with every secret replaced by its state.
type Parsed struct {
	Text        string
	Connections []string
	Secrets     []domain.NewSecret
}

// Parser checks a document as typed. It returns an *domain.InvalidSettingsError for a document with problems.
type Parser interface {
	Parse(text string) (Parsed, error)
}

// Sealer encrypts a secret value for storage.
type Sealer interface {
	Seal(plain []byte) (ciphertext []byte, keyID string, err error)
}

type Clock interface {
	Now() time.Time
}
