package domain

// SpecFile is one file under tasks/ in the team's home. It holds a Spec, or a Problem that says why it
// holds none, so a person can see a broken file instead of a spec that silently went missing.
type SpecFile struct {
	// File is the path in the home, such as tasks/pr-evidence.yaml.
	File    string
	Spec    TaskSpec
	Problem string
	// Commit is the home commit the file was read at, empty when the home has none, such as a plain folder.
	Commit Commit
}

// Valid is true when the file holds a spec.
func (f SpecFile) Valid() bool { return f.Problem == "" }
