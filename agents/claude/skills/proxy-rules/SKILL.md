---
name: proxy-rules
description: Use when asked to let a repo's Boxes make a request that leaves without its secret, to add, remove or change a line of a repo's Proxy rules, or when a credential_withheld event names a request the user wants allowed.
---

# Writing a repo's Proxy rules

A repo's Proxy rules are one YAML text in moor. Every Box of the repo follows
them. Each rule names its `hosts`, the `credential` it injects, and `allow`
lines of the form `"METHOD path"`. A request to a rule's host that matches no
line leaves without the secret. moor then records a `credential_withheld`
event, and its `shape` is the request as the rule saw it.

Write only what the user asked for. Never add a host, a credential or a
method they did not name, and never widen a line on your own.

1. Read the rules with `get_repo_config`, passing `repo` and `file: "rules"`.
   Keep the `revision` it returns. Without the MCP tools, run
   `moor repo rules get <owner/name> --json`.
2. Find the request. When the user names a withheld request, take its
   `shape` and `host` from the `credential_withheld` events of the repo's
   Boxes, or from the list on the repo's page.
3. Write the narrowest line that matches it, under the rule whose `hosts`
   holds that host. In a path, `{repo}` is the Box's own repo, `{any_repo}`
   is any repo, `*` is one segment and `**` is the rest. Prefer `{repo}`
   to `{any_repo}` and `*` to `**`. For `POST /graphql`, add the field
   name to its `query` or `mutation` list.
4. Keep every other line, comment and the order as they are.
5. Save the whole text with `set_repo_config`, passing `repo`,
   `file: "rules"`, `text` and `expected_revision`. Without the MCP tools,
   run `moor repo rules set <owner/name> --expected-revision <n> -f <file>`.
   If it is refused as stale, read the rules again and redo your edit. If the
   text does not parse, fix only your edit.
6. Tell the user each line you added or removed, word for word, and the host
   it sits under. A running Box follows the new rules from its next command.

Saving needs a GitHub account that can push to the repo.
