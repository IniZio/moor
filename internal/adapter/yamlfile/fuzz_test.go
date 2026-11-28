package yamlfile_test

import (
	"fmt"
	"math/rand"
	"reflect"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/adapter/yamlfile"
)

func seeds() [][]byte {
	return [][]byte{
		exampleSpec, exampleSettings, []byte(commented), []byte(skeleton), nil, []byte("---\n---\n"), []byte("&a [*a]"),
		[]byte("name: \"\\x00\"\n"), []byte("on: {on: {on: {}}}"), []byte(strings.Repeat("[", 5000)), []byte("\xff\xfe\x00"),
		[]byte("prompt: ${{"), []byte("a: !!binary x\n"), []byte("? [a]\n: b\n"), []byte("- - - - a\n"), []byte("\xef\xbb\xbfname: a\n"),
	}
}

func checkSpec(t testing.TB, data []byte) {
	spec, err := yamlfile.ParseTaskSpec(data)
	if err == nil && spec.Validate() != nil {
		t.Fatalf("a spec that parsed does not validate: %q", data)
	}
}

func checkSettings(t testing.TB, data []byte) {
	doc, _, err := yamlfile.ParseSettings(data)
	if err != nil {
		return
	}
	if err := doc.Settings().Validate(); err != nil {
		t.Fatalf("settings that parsed do not validate: %v: %q", err, data)
	}
	out, err := doc.Render()
	if err != nil {
		t.Fatalf("render: %v: %q", err, data)
	}
	again, more, err := yamlfile.ParseSettings(out)
	if err != nil || len(more) != 0 || !reflect.DeepEqual(again.Settings(), doc.Settings()) {
		t.Fatalf("round trip failed: %v %v: %q -> %q", err, more, data, out)
	}
}

func FuzzParseTaskSpec(f *testing.F) {
	for _, s := range seeds() {
		f.Add(s)
	}
	f.Fuzz(func(t *testing.T, data []byte) { checkSpec(t, data) })
}

func FuzzParseSettings(f *testing.F) {
	for _, s := range seeds() {
		f.Add(s)
	}
	f.Fuzz(func(t *testing.T, data []byte) { checkSettings(t, data) })
}

// TestParsersSurviveArbitraryBytes runs the fuzz checks over random bytes and random edits of the seeds on
// every go test, which the fuzz targets alone only do for their seeds.
func TestParsersSurviveArbitraryBytes(t *testing.T) {
	rng := rand.New(rand.NewSource(7))
	all := seeds()
	const noise = "\n- {:\"'${}#&*! "
	for i := 0; i < 3000; i++ {
		var data []byte
		if i%3 == 0 {
			data = make([]byte, rng.Intn(200))
			rng.Read(data)
		} else {
			data = append([]byte(nil), all[rng.Intn(len(all))]...)
			for n := rng.Intn(6); n > 0 && len(data) > 0; n-- {
				at := rng.Intn(len(data))
				switch rng.Intn(3) {
				case 0:
					data[at] = byte(rng.Intn(256))
				case 1:
					data = append(data[:at], data[at+1:]...)
				default:
					data = append(data[:at], append([]byte(noise[rng.Intn(len(noise)):]), data[at:]...)...)
				}
			}
		}
		checkSpec(t, data)
		checkSettings(t, data)
	}
}

// TestRenderNeverHoldsAGeneratedSecret writes each secret in every scalar style and checks no part of it survives.
func TestRenderNeverHoldsAGeneratedSecret(t *testing.T) {
	rng := rand.New(rand.NewSource(11))
	styles := []string{"%s", "%q", "'%s'", "|\n      %s\n      %s-more", ">\n      %s", "%s # %s"}
	for i := 0; i < 500; i++ {
		secret := fmt.Sprintf("Zq%08xK%d", rng.Uint32(), rng.Intn(1000))
		style := styles[rng.Intn(len(styles))]
		value := strings.ReplaceAll(style, "%s", secret)
		if style == "%q" {
			value = fmt.Sprintf("%q", secret)
		}
		in := yml("# note", "connections:", "  - name: C", "    hosts: [a.example]", "    add: placeholder T", "    value: "+value, "  - name: D", "    hosts: [b.example]", "    add: placeholder U", "    value: set")
		doc, secrets, err := yamlfile.ParseSettings(in)
		if err != nil || len(secrets) != 1 || !strings.HasPrefix(secrets[0].Value, secret) {
			t.Fatalf("%q: %v %v", in, err, secrets)
		}
		out, err := doc.Render()
		if err != nil || strings.Contains(string(out), "Zq") {
			t.Fatalf("%q rendered %q: %v", in, out, err)
		}
	}
}
