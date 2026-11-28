package domain

// shortCommit is how many characters of a commit hash a person reads.
const shortCommit = 7

// Short is the start of the hash that screens show.
func (c Commit) Short() string {
	if len(c) <= shortCommit {
		return string(c)
	}
	return string(c[:shortCommit])
}
