package settings

import (
	"context"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

// Store is the calls the service makes on the stored versions, secrets and blocked requests.
type Store interface {
	LatestSettings(ctx context.Context) (domain.SettingsVersion, error)
	SettingsVersions(ctx context.Context) ([]domain.SettingsVersion, error)
	SaveSettings(ctx context.Context, w domain.SettingsWrite) (domain.SettingsVersion, error)
	Secret(ctx context.Context, connection string) (domain.SealedSecret, error)
	// RecordBlocked adds a row for a new proxy, host, port and method of the task, or counts one more on it.
	RecordBlocked(ctx context.Context, h domain.BlockedHit) error
	BlockedRequests(ctx context.Context, task domain.TaskID) ([]domain.BlockedRequest, error)
	BlockedRequest(ctx context.Context, id domain.BlockedID) (domain.BlockedRequest, error)
	DecideBlocked(ctx context.Context, id domain.BlockedID, d domain.Decision, by string, at time.Time) error
}

// Parsed is a document that passed the parser. Text is the document with every secret replaced by its state,
// Repositories are the ones it allows and Settings is the whole document as typed settings.
type Parsed struct {
	Text         string
	Repositories []domain.Repo
	Connections  []string
	Secrets      []domain.NewSecret
	Settings     domain.Settings
}

// Parser checks a document as typed. It returns an *domain.InvalidSettingsError for a document with problems.
// AppendAllow adds an allow line to a stored document and says which line holds it.
type Parser interface {
	Parse(text string) (Parsed, error)
	AppendAllow(text, rule string) (string, int, error)
}

// Sealer encrypts a secret value for storage and opens it again for the proxy that adds it.
type Sealer interface {
	Seal(plain []byte) (ciphertext []byte, keyID string, err error)
	Open(sealed []byte, keyID string) ([]byte, error)
}

type Clock interface {
	Now() time.Time
}
