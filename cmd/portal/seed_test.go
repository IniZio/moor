package main

import (
	"bytes"
	"context"
	"encoding/base64"
	"errors"
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
)

const seedSecret = "seeded-token-5c1e"

func seedDoc() string { return strings.Replace(goodDoc, "value: not set", "value: "+seedSecret, 1) }

func writeSeed(t *testing.T, text string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "settings.yaml")
	if err := os.WriteFile(path, []byte(text), 0o600); err != nil {
		t.Fatal(err)
	}
	return path
}

func withSeed(path string) func(*config) { return func(c *config) { c.seedSettingsFile = path } }

func versionsOf(t *testing.T, dsn, schema string) []domain.SettingsVersion {
	t.Helper()
	db, err := store.OpenIn(context.Background(), dsn, schema)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	vs, err := db.SettingsVersions(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	return vs
}

func captureLog(t *testing.T) *bytes.Buffer {
	t.Helper()
	var buf bytes.Buffer
	log.SetOutput(&buf)
	t.Cleanup(func() { log.SetOutput(os.Stderr) })
	return &buf
}

func TestAPortalWithNoSettingsSavesTheSeedAsVersionOneLikeAPersonWould(t *testing.T) {
	logs := captureLog(t)
	a := newApp(t, withSeed(writeSeed(t, seedDoc())))
	vs := versionsOf(t, a.dsn, "")
	if len(vs) != 1 || vs[0].N != 1 || vs[0].SavedBy != "seed" {
		t.Fatalf("versions = %+v, want version 1 saved by seed", vs)
	}
	if !strings.Contains(vs[0].Text, "value: set") || strings.Contains(vs[0].Text, seedSecret) {
		t.Errorf("the stored document is not the editor's form: %s", vs[0].Text)
	}
	a.requireSealed("shop-staging/value", seedSecret)
	for _, path := range []string{"/api/settings", "/api/settings/versions", "/settings"} {
		a.do(get("Mia", path)).lacks(t, seedSecret)
	}
	a.do(get("Mia", "/api/settings/versions")).has(t, 200, "seed", "replaced the value of shop-staging/value")
	if strings.Contains(logs.String(), seedSecret) {
		t.Errorf("the log holds the secret: %s", logs)
	}
}

func TestAPortalWithSettingsLeavesThemAndNeverReadsTheSeed(t *testing.T) {
	logs := captureLog(t)
	a := newApp(t)
	a.do(put("Mia", 0, goodDoc)).has(t, 200, `"version":1`)
	for name, path := range map[string]string{"another document": writeSeed(t, seedDoc()), "no file": filepath.Join(t.TempDir(), "gone.yaml"), "a broken file": writeSeed(t, "{{ not yaml "+seedSecret)} {
		b := newAppOn(t, a.dsn, withSeed(path))
		if got := b.current(); got.Version != 1 || strings.Contains(got.Text, seedSecret) {
			t.Errorf("%s: current = %+v", name, got)
		}
	}
	if vs := versionsOf(t, a.dsn, ""); len(vs) != 1 || vs[0].SavedBy != "Mia" {
		t.Errorf("versions = %+v, want Mia's version 1 alone", vs)
	}
	if strings.Contains(logs.String(), seedSecret) {
		t.Errorf("the log holds the secret: %s", logs)
	}
}

func TestAnInvalidSeedStopsTheStartWithTheProblemAndNoSecret(t *testing.T) {
	dsn := pgtest.SchemaDSN(t)
	bad := map[string]string{
		"an unknown key": seedDoc() + "extra: " + seedSecret + "\n",
		"a bad rule":     strings.Replace(seedDoc(), "hosts: [api.example.com]", "hosts: [api_example.com]", 1),
		"not yaml":       "repositories: [" + seedSecret + "\n  - : :\n",
		"wrong shape":    "credentials: " + seedSecret + "\n",
	}
	for name, text := range bad {
		cfg := testConfig(t, dsn)
		cfg.seedSettingsFile = writeSeed(t, text)
		_, _, err := build(context.Background(), cfg)
		if err == nil {
			t.Errorf("%s: the portal started", name)
			continue
		}
		if strings.Contains(err.Error(), seedSecret) {
			t.Errorf("%s: the refusal echoes the secret: %v", name, err)
		}
		if !strings.Contains(err.Error(), "seed settings") {
			t.Errorf("%s: the refusal does not say where it comes from: %v", name, err)
		}
	}
	if vs := versionsOf(t, dsn, ""); len(vs) != 0 {
		t.Errorf("a refused seed made %+v", vs)
	}
	cfg := testConfig(t, dsn)
	cfg.seedSettingsFile = filepath.Join(t.TempDir(), "gone.yaml")
	if _, _, err := build(context.Background(), cfg); err == nil || !strings.Contains(err.Error(), "gone.yaml") {
		t.Errorf("a missing file: %v", err)
	}
}

func testConfig(t *testing.T, dsn string) config {
	t.Helper()
	key, err := base64.StdEncoding.DecodeString(testKeyB64)
	if err != nil {
		t.Fatal(err)
	}
	return config{listen: "127.0.0.1:0", database: dsn, userHeader: userHeader, orgName: "example-org", secretKey: key}
}

// stale stands in for a settings service that finds none, and then finds another replica's version 1 at the save.
type stale struct{ saves int }

func (s *stale) Current(context.Context) (domain.SettingsVersion, error) {
	return domain.SettingsVersion{}, nil
}

func (s *stale) Save(context.Context, domain.SettingsSave) (domain.SettingsSaved, error) {
	s.saves++
	return domain.SettingsSaved{}, &domain.StaleSettingsError{Newest: 1}
}

func TestTheReplicaThatLosesTheRaceForVersionOneIsDone(t *testing.T) {
	svc := &stale{}
	if err := seedSettings(context.Background(), svc, writeSeed(t, goodDoc)); err != nil || svc.saves != 1 {
		t.Fatalf("seed = %v after %d saves", err, svc.saves)
	}
	if err := seedSettings(context.Background(), &failing{}, writeSeed(t, goodDoc)); err == nil || !errors.Is(err, errDown) {
		t.Fatalf("a failed save was taken for done: %v", err)
	}
}

var errDown = errors.New("database down")

type failing struct{ stale }

func (f *failing) Save(context.Context, domain.SettingsSave) (domain.SettingsSaved, error) {
	return domain.SettingsSaved{}, errDown
}

// startTogether builds n portals on one config at the same moment and gives their handlers.
func startTogether(t *testing.T, n int, cfg config) []*app {
	t.Helper()
	apps := make([]*app, n)
	errs := make([]error, n)
	var wg sync.WaitGroup
	start := make(chan struct{})
	for i := range n {
		wg.Add(1)
		go func() {
			defer wg.Done()
			<-start
			h, closeApp, err := build(context.Background(), cfg)
			errs[i] = err
			apps[i] = &app{t: t, handler: h, dsn: cfg.database, close: closeApp}
		}()
	}
	close(start)
	wg.Wait()
	for i, err := range errs {
		if err != nil {
			t.Fatalf("replica %d: %v", i, err)
		}
		t.Cleanup(apps[i].close)
	}
	return apps
}

// Replicas of one deploy start together on a schema nobody has made: one version 1, every replica configured.
func TestReplicasStartingTogetherOnAFreshSchemaMakeOneVersionOne(t *testing.T) {
	dsn, schema := pgtest.BaseDSN(t), pgtest.Name(t)
	cfg := testConfig(t, dsn)
	cfg.databaseSchema, cfg.seedSettingsFile = schema, writeSeed(t, seedDoc())
	apps := startTogether(t, 6, cfg)
	vs := versionsOf(t, dsn, schema)
	if len(vs) != 1 || vs[0].N != 1 || vs[0].SavedBy != "seed" {
		t.Fatalf("versions = %+v, want one version 1 saved by seed", vs)
	}
	for i, a := range apps {
		if got := a.current(); got.Version != 1 || !strings.Contains(got.Text, "example-org/shop-web") {
			t.Errorf("replica %d serves %+v", i, got)
		}
	}
}

func TestStorageFlagsAndTheirVariables(t *testing.T) {
	c, err := parseConfig(nil, env(goodEnv()))
	if err != nil || c.databaseSchema != "" || c.seedSettingsFile != "" {
		t.Fatalf("defaults = %+v, %v", c, err)
	}
	e := goodEnv()
	e["PORTAL_DATABASE_SCHEMA"], e["PORTAL_SEED_SETTINGS_FILE"] = "moor", "/etc/portal/seed/settings.yaml"
	c, err = parseConfig(nil, env(e))
	if err != nil || c.databaseSchema != "moor" || c.seedSettingsFile != "/etc/portal/seed/settings.yaml" {
		t.Fatalf("environment = %+v, %v", c, err)
	}
	c, err = parseConfig([]string{"-database-schema", "other", "-seed-settings-file", "/tmp/s.yaml"}, env(e))
	if err != nil || c.databaseSchema != "other" || c.seedSettingsFile != "/tmp/s.yaml" {
		t.Fatalf("flags = %+v, %v", c, err)
	}
	e["PORTAL_DATABASE_SCHEMA"] = "Not Plain; DROP"
	if _, err := parseConfig(nil, env(e)); err == nil || !strings.Contains(err.Error(), "database-schema") {
		t.Errorf("a bad schema name: %v", err)
	}
}
