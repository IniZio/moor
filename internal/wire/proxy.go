package wire

// ProxyAccess is how a task's two proxies reach the portal: its address, and a token for each proxy that
// names the task and the proxy. Only the proxies hold the proxy tokens, never the Box or the desk. TaskToken is
// the task scope's token, which the desk's publish tool presents, and it opens nothing a proxy's token does.
type ProxyAccess struct {
	Portal    string `json:"portal"`
	BoxToken  string `json:"box_token"`
	DeskToken string `json:"desk_token"`
	TaskToken string `json:"task_token,omitempty"`
}

// Policy answers GET /internal/policy/{task}: one settings version's allow lines and the connections the
// asking proxy adds, without their values. Lines and add rules are in the settings document's own text.
type Policy struct {
	Version     int                `json:"version"`
	Allow       []string           `json:"allow"`
	Connections []PolicyConnection `json:"connections"`
}

type PolicyConnection struct {
	Name  string   `json:"name"`
	Hosts []string `json:"hosts"`
	Add   string   `json:"add"`
	Set   bool     `json:"set"`
}

// ValueRequest asks POST /internal/connections/value for one connection's value.
type ValueRequest struct {
	Task       string `json:"task"`
	Connection string `json:"connection"`
}

type ValueReply struct {
	Value string `json:"value"`
}

// BlockedReport tells POST /internal/blocked about one request a proxy refused.
type BlockedReport struct {
	Task   string `json:"task"`
	Host   string `json:"host"`
	Port   int    `json:"port"`
	Method string `json:"method"`
	Reason string `json:"reason"`
}
