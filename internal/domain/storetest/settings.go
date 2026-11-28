package storetest

import (
	"context"
	"errors"
	"reflect"
	"sync"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

// SettingsStore is the calls the settings logic makes. It mirrors the interface in package settings,
// which this package cannot import.
type SettingsStore interface {
	// LatestSettings returns domain.ErrNotFound while no version exists.
	LatestSettings(ctx context.Context) (domain.SettingsVersion, error)
	// SettingsVersions lists every version, newest first.
	SettingsVersions(ctx context.Context) ([]domain.SettingsVersion, error)
	// SaveSettings returns domain.ErrStaleSettings for a base that is not the newest version.
	SaveSettings(ctx context.Context, w domain.SettingsWrite) (domain.SettingsVersion, error)
	// Secret returns domain.ErrNotFound for a connection with no stored value.
	Secret(ctx context.Context, connection string) (domain.SealedSecret, error)
}

// RunSettings runs every settings case on a fresh store.
func RunSettings(t *testing.T, fresh func(t *testing.T) SettingsStore) {
	t.Helper()
	cases := []struct {
		name string
		run  func(*testing.T, SettingsStore)
	}{
		{"no version yet", noSettings},
		{"round trip", settingsRoundTrip},
		{"versions list newest first", settingsOrder},
		{"stale base", staleSettings},
		{"a stale save stores no secret", staleSettingsKeepsSecrets},
		{"saves that race on one base", racingSettings},
		{"secrets follow the saves", settingsSecrets},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) { c.run(t, fresh(t)) })
	}
}

func write(base int, text string) domain.SettingsWrite {
	return domain.SettingsWrite{Base: base, Text: text, By: "ann", At: at(int64(base + 1))}
}

func saveSettings(t *testing.T, s SettingsStore, w domain.SettingsWrite) domain.SettingsVersion {
	t.Helper()
	v, err := s.SaveSettings(context.Background(), w)
	if err != nil {
		t.Fatalf("save on base %d: %v", w.Base, err)
	}
	return v
}

func wantStale(t *testing.T, what string, err error) {
	t.Helper()
	if !errors.Is(err, domain.ErrStaleSettings) {
		t.Errorf("%s: error %v, want ErrStaleSettings", what, err)
	}
}

func noSettings(t *testing.T, s SettingsStore) {
	ctx := context.Background()
	if _, err := s.LatestSettings(ctx); !errors.Is(err, domain.ErrNotFound) {
		t.Errorf("latest of none: %v, want ErrNotFound", err)
	}
	if v, err := s.SettingsVersions(ctx); err != nil || len(v) != 0 {
		t.Errorf("versions of none = %v, %v", v, err)
	}
	if _, err := s.Secret(ctx, "shop staging API"); !errors.Is(err, domain.ErrNotFound) {
		t.Errorf("secret of none: %v, want ErrNotFound", err)
	}
}

func settingsRoundTrip(t *testing.T, s SettingsStore) {
	w := write(0, "repositories:\n  - acme/shop\n")
	w.Note = "first"
	want := domain.SettingsVersion{N: 1, Text: w.Text, Note: "first", SavedBy: "ann", SavedAt: at(1)}
	if got := saveSettings(t, s, w); !reflect.DeepEqual(got, want) {
		t.Errorf("saved %+v, want %+v", got, want)
	}
	got, err := s.LatestSettings(context.Background())
	if err != nil || !reflect.DeepEqual(got, want) {
		t.Errorf("latest %+v, %v, want %+v", got, err, want)
	}
}

func settingsOrder(t *testing.T, s SettingsStore) {
	saveSettings(t, s, write(0, "a\n"))
	saveSettings(t, s, write(1, "b\n"))
	v3 := write(2, "c\n")
	v3.By, v3.Note = "bob", "why"
	saveSettings(t, s, v3)
	got, err := s.SettingsVersions(context.Background())
	if err != nil || len(got) != 3 {
		t.Fatalf("versions = %v, %v", got, err)
	}
	for i, want := range []struct {
		n    int
		text string
		by   string
		note string
	}{{3, "c\n", "bob", "why"}, {2, "b\n", "ann", ""}, {1, "a\n", "ann", ""}} {
		if v := got[i]; v.N != want.n || v.Text != want.text || v.SavedBy != want.by || v.Note != want.note || !v.SavedAt.Equal(at(int64(want.n))) {
			t.Errorf("versions[%d] = %+v, want %+v", i, v, want)
		}
	}
}

func staleSettings(t *testing.T, s SettingsStore) {
	ctx := context.Background()
	_, err := s.SaveSettings(ctx, write(1, "x\n"))
	wantStale(t, "base 1 on an empty store", err)
	saveSettings(t, s, write(0, "a\n"))
	_, err = s.SaveSettings(ctx, write(0, "b\n"))
	wantStale(t, "base 0 after a save", err)
	_, err = s.SaveSettings(ctx, write(5, "b\n"))
	wantStale(t, "base ahead of the newest", err)
	if got, _ := s.SettingsVersions(ctx); len(got) != 1 || got[0].Text != "a\n" {
		t.Errorf("a refused save changed the versions: %+v", got)
	}
}

func sealed(connection, cipher string, n int64) domain.SealedSecret {
	return domain.SealedSecret{Connection: connection, Ciphertext: []byte(cipher), KeyID: "k1", UpdatedAt: at(n)}
}

func staleSettingsKeepsSecrets(t *testing.T, s SettingsStore) {
	saveSettings(t, s, write(0, "a\n"))
	w := write(0, "b\n")
	w.Keep, w.Secrets = []string{"api"}, []domain.SealedSecret{sealed("api", "c1", 2)}
	_, err := s.SaveSettings(context.Background(), w)
	wantStale(t, "stale save with a secret", err)
	if _, err := s.Secret(context.Background(), "api"); !errors.Is(err, domain.ErrNotFound) {
		t.Errorf("a refused save stored its secret: %v", err)
	}
}

func racingSettings(t *testing.T, s SettingsStore) {
	const racers = 8
	errs := make([]error, racers)
	start := make(chan struct{})
	var wg sync.WaitGroup
	for i := range racers {
		wg.Go(func() {
			<-start
			_, errs[i] = s.SaveSettings(context.Background(), write(0, "text\n"))
		})
	}
	close(start)
	wg.Wait()
	won := 0
	for _, err := range errs {
		switch {
		case err == nil:
			won++
		case !errors.Is(err, domain.ErrStaleSettings):
			t.Errorf("racing save: %v, want success or ErrStaleSettings", err)
		}
	}
	if got, _ := s.SettingsVersions(context.Background()); won != 1 || len(got) != 1 {
		t.Errorf("%d saves won and %d versions exist, want one of each", won, len(got))
	}
}

func settingsSecrets(t *testing.T, s SettingsStore) {
	ctx := context.Background()
	w := write(0, "a\n")
	w.Keep, w.Secrets = []string{"api", "docs"}, []domain.SealedSecret{sealed("api", "c1", 1), sealed("docs", "d1", 1)}
	saveSettings(t, s, w)
	w = write(1, "b\n")
	w.Keep, w.Secrets = []string{"api", "docs"}, []domain.SealedSecret{sealed("api", "c2", 2)}
	saveSettings(t, s, w)
	for conn, want := range map[string]domain.SealedSecret{"api": sealed("api", "c2", 2), "docs": sealed("docs", "d1", 1)} {
		if got, err := s.Secret(ctx, conn); err != nil || !reflect.DeepEqual(got, want) {
			t.Errorf("secret %s = %+v, %v, want %+v", conn, got, err, want)
		}
	}
	w = write(2, "c\n")
	w.Keep = []string{"docs"}
	saveSettings(t, s, w)
	if _, err := s.Secret(ctx, "api"); !errors.Is(err, domain.ErrNotFound) {
		t.Errorf("a connection left out of the newest version kept its secret: %v", err)
	}
	if _, err := s.Secret(ctx, "docs"); err != nil {
		t.Errorf("a kept connection lost its secret: %v", err)
	}
}
