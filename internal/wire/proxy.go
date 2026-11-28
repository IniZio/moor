package wire

import "time"

// ProxyAccess is how a task's two proxies reach the portal: its address, and a token for each proxy that
// names the task and the proxy. Only the proxies hold the proxy tokens, never the Box or the desk. TaskToken is
// the task scope's token, which the desk proxy puts on the desk's publish calls, and it opens nothing a proxy's
// token does.
type ProxyAccess struct {
	Portal    string `json:"portal"`
	BoxToken  string `json:"box_token"`
	DeskToken string `json:"desk_token"`
	TaskToken string `json:"task_token,omitempty"`
}

// Policy answers GET /internal/policy/{task}: one settings version's rules for the asking proxy, without any
// secret, and the task's repository, which {repo} stands for. Hosts and entries are in the document's own text,
// and an allow of ["*"] lets every request through.
type Policy struct {
	Version int          `json:"version"`
	Repo    Repo         `json:"repo"`
	Rules   []PolicyRule `json:"rules"`
}

type PolicyRule struct {
	Name             string   `json:"name"`
	Hosts            []string `json:"hosts"`
	Credential       string   `json:"credential,omitempty"`
	Set              bool     `json:"set"`
	Inject           *Inject  `json:"inject,omitempty"`
	Allow            []string `json:"allow"`
	Ask              []string `json:"ask,omitempty"`
	GraphQLMutations []string `json:"graphql_mutations,omitempty"`
}

// Inject is a header and its form, or a Basic user.
type Inject struct {
	Header    string `json:"header,omitempty"`
	Form      string `json:"form,omitempty"`
	BasicUser string `json:"basic_user,omitempty"`
}

// EnvVar is one variable the settings' rules set in the Box. A placeholder holds a fake token, never a secret.
type EnvVar struct {
	Name  string `json:"name"`
	Value string `json:"value"`
}

// ValueRequest asks POST /internal/credentials/value for what one credential adds.
type ValueRequest struct {
	Task       string `json:"task"`
	Credential string `json:"credential"`
}

// ValueReply is the value, and when it expires if it is a minted token.
type ValueReply struct {
	Value   string    `json:"value"`
	Expires time.Time `json:"expires,omitzero"`
}

// BlockedReport tells POST /internal/blocked about one request a proxy refused.
type BlockedReport struct {
	Task   string `json:"task"`
	Host   string `json:"host"`
	Port   int    `json:"port"`
	Method string `json:"method"`
	Path   string `json:"path,omitempty"`
	Rule   string `json:"rule,omitempty"`
	Entry  string `json:"entry,omitempty"`
	Reason string `json:"reason"`
	Detail string `json:"detail,omitempty"`
}
