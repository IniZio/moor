package settings_test

import (
	"context"
	"errors"
	"fmt"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/settings"
	"github.com/IniZio/moor/internal/settings/settingstest"
)

// stubParser reads a document as lines. A line "bad" is a problem on that line, a line "secret NAME VALUE" is a
// typed value of the credential NAME that the parsed text shows as "NAME: set", "cred NAME STATE" is a credential,
// "app NAME STATE" a GitHub App credential, and "rule NAME HOST CRED" a rule that adds CRED and allows any request
// to HOST, with - for no credential. An underscore in STATE stands for a space.
type stubParser struct{}

func (stubParser) Parse(text string) (settings.Parsed, error) {
	var p settings.Parsed
	var bad []domain.SettingsProblem
	var kept []string
	for i, line := range strings.Split(strings.TrimSuffix(text, "\n"), "\n") {
		f := strings.Fields(line)
		switch {
		case line == "bad":
			bad = append(bad, domain.SettingsProblem{Line: i + 1, Message: "this line is bad"})
		case len(f) == 2 && f[0] == "repo":
			repo, err := domain.ParseRepo(f[1])
			if err != nil {
				return settings.Parsed{}, fmt.Errorf("stub repo %q: %w", f[1], err)
			}
			p.Repositories = append(p.Repositories, repo)
			kept = append(kept, line)
		case len(f) == 3 && f[0] == "secretkey":
			p.Secrets = append(p.Secrets, domain.NewSecret{Key: f[1], Value: f[2]})
			kept = append(kept, f[1]+": set")
		case len(f) == 3 && f[0] == "secret":
			p.Secrets = append(p.Secrets, domain.NewSecret{Key: domain.SecretKey(f[1], domain.FieldValue), Value: f[2]})
			p.Settings.Credentials = append(p.Settings.Credentials, domain.Credential{Name: f[1], Value: domain.SecretSet})
			kept = append(kept, f[1]+": set")
		default:
			stubAccess(&p, f)
			kept = append(kept, line)
		}
	}
	if len(bad) > 0 {
		return settings.Parsed{}, &domain.InvalidSettingsError{Problems: bad}
	}
	p.Text = strings.Join(kept, "\n") + "\n"
	return p, nil
}

func stubAccess(p *settings.Parsed, f []string) {
	state := func(s string) domain.SecretState { return domain.SecretState(strings.ReplaceAll(s, "_", " ")) }
	switch {
	case len(f) == 3 && f[0] == "cred":
		p.Settings.Credentials = append(p.Settings.Credentials, domain.Credential{Name: f[1], Value: state(f[2])})
	case len(f) == 3 && f[0] == "app":
		app := &domain.GitHubApp{AppID: 7, PrivateKey: state(f[2]), WebhookSecret: domain.SecretNotSet}
		p.Settings.Credentials = append(p.Settings.Credentials, domain.Credential{Name: f[1], App: app})
	case len(f) == 4 && f[0] == "rule":
		r := domain.Rule{Name: f[1], Hosts: []domain.HostPort{{Host: domain.HostPattern(f[2]), Port: 443}}, Allow: domain.Access{Any: true}}
		if f[3] != "-" {
			r.Credential, r.Inject = f[3], domain.HeaderInject{Name: "X-Key", Form: domain.SecretMark}
		}
		p.Settings.Rules = append(p.Settings.Rules, r)
	}
}

// AllowEntry adds an "entry RULE-OR-HOST ENTRY" line at the end, or answers the line that already says it.
func (stubParser) AllowEntry(text, rule, entry string, host domain.HostPort) (string, int, error) {
	where := rule
	if where == "" {
		where = host.String()
	}
	want := "entry " + where + " " + entry
	lines := strings.Split(strings.TrimSuffix(text, "\n"), "\n")
	for i, l := range lines {
		if l == want {
			return text, i + 1, nil
		}
	}
	lines = append(lines, want)
	return strings.Join(lines, "\n") + "\n", len(lines), nil
}

// minter mints a token named after the repository, and keeps what it was asked.
type minter struct{ asked []string }

func (m *minter) Mint(_ context.Context, app domain.AppKeys, repo domain.Repo) (domain.CredentialValue, error) {
	m.asked = append(m.asked, fmt.Sprintf("app %d key %s for %s", app.AppID, app.PrivateKey, repo.Path))
	return domain.CredentialValue{Value: "minted-" + string(repo.Path), Expires: time.Unix(1_700_003_600, 0)}, nil
}

type reversingSealer struct{ fail bool }

func (reversingSealer) Open(sealed []byte, keyID string) ([]byte, error) {
	if keyID != "k1" {
		return nil, errors.New("another key")
	}
	return []byte(strings.ToLower(string(sealed))), nil
}

func (s reversingSealer) Seal(plain []byte) ([]byte, string, error) {
	if s.fail {
		return nil, "", errors.New("no key")
	}
	out := []byte(strings.ToUpper(string(plain)))
	return out, "k1", nil
}

type clock struct{ now time.Time }

func (c *clock) Now() time.Time { return c.now }

type fixture struct {
	svc    *settings.Service
	store  *settingstest.Memory
	clock  *clock
	minter *minter
}

func newFixture(sealer reversingSealer) fixture {
	store := settingstest.NewMemory()
	c := &clock{now: time.Unix(1_700_000_000, 0).UTC()}
	m := &minter{}
	return fixture{svc: settings.New(store, stubParser{}, sealer, c, m), store: store, clock: c, minter: m}
}

func (f fixture) save(t *testing.T, base int, text string) domain.SettingsSaved {
	t.Helper()
	got, err := f.svc.Save(context.Background(), domain.SettingsSave{Base: base, Text: text, By: "Mia"})
	if err != nil {
		t.Fatalf("save %q on v%d: %v", text, base, err)
	}
	return got
}

func TestBeforeTheFirstSaveTheDocumentIsTheInitialText(t *testing.T) {
	cur, err := newFixture(reversingSealer{}).svc.Current(context.Background())
	if err != nil || cur.N != 0 || cur.Text != settings.InitialText {
		t.Fatalf("current = %+v, %v", cur, err)
	}
}

func TestAllowedAreTheRepositoriesOfTheNewestVersion(t *testing.T) {
	f := newFixture(reversingSealer{})
	ctx := context.Background()
	if got, err := f.svc.Allowed(ctx); err != nil || len(got) != 0 {
		t.Fatalf("before the first save: %v, %v", got, err)
	}
	f.save(t, 0, "repo example-org/shop-web\nrepo example-org/docs-site\n")
	f.save(t, 1, "repo example-org/shop-web\n")
	got, err := f.svc.Allowed(ctx)
	want := []domain.Repo{{Host: "github.com", Path: "example-org/shop-web"}}
	if err != nil || !reflect.DeepEqual(got, want) {
		t.Fatalf("allowed = %v, %v, want %v", got, err, want)
	}
}

func TestASaveMakesTheNextVersionFromTheParsedText(t *testing.T) {
	f := newFixture(reversingSealer{})
	got := f.save(t, 0, "a\n")
	want := domain.SettingsVersion{N: 1, Text: "a\n", SavedBy: "Mia", SavedAt: f.clock.now}
	if got.Unchanged || !reflect.DeepEqual(got.Version, want) {
		t.Fatalf("saved %+v, want %+v", got, want)
	}
	f.clock.now = f.clock.now.Add(time.Hour)
	if v2 := f.save(t, 1, "a\nb\n").Version; v2.N != 2 || !v2.SavedAt.Equal(f.clock.now) {
		t.Errorf("second save = %+v", v2)
	}
}

func TestASaveFromAnOldVersionIsRefusedAndNamesTheNewest(t *testing.T) {
	f := newFixture(reversingSealer{})
	f.save(t, 0, "a\n")
	f.save(t, 1, "b\n")
	_, err := f.svc.Save(context.Background(), domain.SettingsSave{Base: 1, Text: "c\n", By: "Ken"})
	var stale *domain.StaleSettingsError
	if !errors.As(err, &stale) || stale.Newest != 2 || !errors.Is(err, domain.ErrStaleSettings) {
		t.Fatalf("error %v, want a StaleSettingsError naming v2", err)
	}
	if cur, _ := f.svc.Current(context.Background()); cur.N != 2 || cur.Text != "b\n" {
		t.Errorf("a refused save changed the document: %+v", cur)
	}
}

func TestAnInvalidDocumentIsRefusedWithItsLinesAndStoresNothing(t *testing.T) {
	f := newFixture(reversingSealer{})
	_, err := f.svc.Save(context.Background(), domain.SettingsSave{Base: 0, Text: "a\nbad\nsecret api hunter2\nbad\n"})
	var invalid *domain.InvalidSettingsError
	if !errors.As(err, &invalid) || len(invalid.Problems) != 2 || invalid.Problems[0].Line != 2 || invalid.Problems[1].Line != 4 {
		t.Fatalf("error %v, want problems on lines 2 and 4", err)
	}
	if _, err := f.store.LatestSettings(context.Background()); !errors.Is(err, domain.ErrNotFound) {
		t.Errorf("a refused document made a version: %v", err)
	}
	if _, err := f.store.Secret(context.Background(), "api/value"); !errors.Is(err, domain.ErrNotFound) {
		t.Errorf("a refused document stored a secret: %v", err)
	}
	if got := f.svc.Check("bad\n"); len(got) != 1 || got[0].Line != 1 {
		t.Errorf("check = %+v", got)
	}
	if got := f.svc.Check("fine\n"); len(got) != 0 {
		t.Errorf("check of a good document = %+v", got)
	}
}

func TestATypedValueIsSealedApartAndNeverInTheStoredDocumentOrNote(t *testing.T) {
	f := newFixture(reversingSealer{})
	got := f.save(t, 0, "secret api hunter2\nsecret docs s3cret\n")
	if !reflect.DeepEqual(got.Replaced, []string{"api/value", "docs/value"}) || got.Version.Note != "replaced the value of api/value, docs/value" {
		t.Errorf("saved %+v", got)
	}
	for _, text := range []string{got.Version.Text, got.Version.Note} {
		if strings.Contains(strings.ToLower(text), "hunter2") || strings.Contains(strings.ToLower(text), "s3cret") {
			t.Errorf("a value leaked into %q", text)
		}
	}
	sec, err := f.store.Secret(context.Background(), "api/value")
	if err != nil || string(sec.Ciphertext) != "HUNTER2" || sec.KeyID != "k1" || !sec.UpdatedAt.Equal(f.clock.now) {
		t.Errorf("stored secret = %+v, %v", sec, err)
	}
}

func TestAnExplicitNoteWinsOverTheReplacedValueNote(t *testing.T) {
	f := newFixture(reversingSealer{})
	got, err := f.svc.Save(context.Background(), domain.SettingsSave{Base: 0, Text: "secret api x\n", Note: "rotated", By: "Mia"})
	if err != nil || got.Version.Note != "rotated" {
		t.Fatalf("saved %+v, %v", got, err)
	}
}

func TestAFailedSealStoresNoVersion(t *testing.T) {
	f := newFixture(reversingSealer{fail: true})
	if _, err := f.svc.Save(context.Background(), domain.SettingsSave{Base: 0, Text: "secret api x\n"}); err == nil {
		t.Fatal("saved without a key")
	}
	if _, err := f.store.LatestSettings(context.Background()); !errors.Is(err, domain.ErrNotFound) {
		t.Errorf("a failed seal made a version: %v", err)
	}
}

func TestASaveThatChangesNothingMakesNoVersionUnlessAValueWasTyped(t *testing.T) {
	f := newFixture(reversingSealer{})
	f.save(t, 0, "a\nsecret api x\n")
	same := f.save(t, 1, "a\napi: set\n")
	if !same.Unchanged || same.Version.N != 1 {
		t.Errorf("same document: %+v", same)
	}
	rotated := f.save(t, 1, "a\nsecret api y\n")
	if rotated.Unchanged || rotated.Version.N != 2 || rotated.Version.Text != "a\napi: set\n" {
		t.Errorf("a typed value on the same text: %+v", rotated)
	}
}

func TestASaveDropsTheSecretOfARemovedConnection(t *testing.T) {
	f := newFixture(reversingSealer{})
	f.save(t, 0, "secret api x\n")
	f.save(t, 1, "plain\n")
	if _, err := f.store.Secret(context.Background(), "api/value"); !errors.Is(err, domain.ErrNotFound) {
		t.Errorf("a removed connection kept its secret: %v", err)
	}
}

func TestHistoryListsNewestFirstEachDiffedAgainstTheOneBefore(t *testing.T) {
	f := newFixture(reversingSealer{})
	f.save(t, 0, "a\nb\n")
	f.save(t, 1, "a\nc\n")
	f.save(t, 2, "a\nc\nd\n")
	got, err := f.svc.History(context.Background())
	if err != nil || len(got) != 3 {
		t.Fatalf("history = %v, %v", got, err)
	}
	type line = domain.DiffLine
	for i, want := range [][]line{
		{{Kind: domain.DiffSame, Text: "a"}, {Kind: domain.DiffSame, Text: "c"}, {Kind: domain.DiffAdded, Text: "d"}},
		{{Kind: domain.DiffSame, Text: "a"}, {Kind: domain.DiffRemoved, Text: "b"}, {Kind: domain.DiffAdded, Text: "c"}},
		{{Kind: domain.DiffAdded, Text: "a"}, {Kind: domain.DiffAdded, Text: "b"}},
	} {
		if got[i].N != 3-i || !reflect.DeepEqual(got[i].Diff, want) {
			t.Errorf("history[%d] = v%d %+v, want %+v", i, got[i].N, got[i].Diff, want)
		}
	}
}

func TestADiffOfTwoEqualTextsKeepsEveryLine(t *testing.T) {
	got := settings.Diff("a\nb\n", "a\nb\n")
	if len(got) != 2 || got[0].Kind != domain.DiffSame || got[1].Text != "b" {
		t.Errorf("diff = %+v", got)
	}
	if got := settings.Diff("", ""); len(got) != 0 {
		t.Errorf("diff of nothing = %+v", got)
	}
}
