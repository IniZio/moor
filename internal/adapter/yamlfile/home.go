package yamlfile

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"path"
	"strings"

	"github.com/IniZio/moor/internal/domain"
)

const specDir = "tasks"

// Home reads the task specs of the team's home, which is a checked-out folder with one tasks/*.yaml file
// per spec. It reads on every call, so a pull of the home shows at once.
type Home struct{ files fs.FS }

// NewHome reads the specs under tasks/ of files, the root of the home.
func NewHome(files fs.FS) Home { return Home{files: files} }

// TaskSpecs lists every tasks/*.yaml file by file name. A file that cannot be read or parsed is listed with
// its problem, and so is a file whose name another file already has, compared without case.
func (h Home) TaskSpecs(_ context.Context) ([]domain.SpecFile, error) {
	entries, err := fs.ReadDir(h.files, specDir)
	if errors.Is(err, fs.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("list %s: %w", specDir, err)
	}
	var out []domain.SpecFile
	firstWith := map[string]string{}
	for _, e := range entries {
		if !e.IsDir() && strings.HasSuffix(e.Name(), ".yaml") {
			out = append(out, uniqueName(h.read(path.Join(specDir, e.Name())), firstWith))
		}
	}
	return out, nil
}

// uniqueName turns a file whose spec name an earlier file has into a problem entry. firstWith remembers,
// by lower-cased name, the file that has each name first.
func uniqueName(f domain.SpecFile, firstWith map[string]string) domain.SpecFile {
	if !f.Valid() {
		return f
	}
	key := strings.ToLower(string(f.Spec.Name))
	first, dup := firstWith[key]
	if !dup {
		firstWith[key] = f.File
		return f
	}
	return domain.SpecFile{File: f.File, Problem: fmt.Sprintf("the name %q is already used by %s", f.Spec.Name, first)}
}

func (h Home) read(file string) domain.SpecFile {
	data, err := fs.ReadFile(h.files, file)
	if err != nil {
		return domain.SpecFile{File: file, Problem: "the file could not be read: " + err.Error()}
	}
	spec, err := ParseTaskSpec(data)
	if err != nil {
		return domain.SpecFile{File: file, Problem: strings.TrimPrefix(err.Error(), ErrInvalid.Error()+": ")}
	}
	return domain.SpecFile{File: file, Spec: spec}
}
