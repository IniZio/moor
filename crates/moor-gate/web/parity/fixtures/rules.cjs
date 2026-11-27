'use strict';
const path = require('path');
const fs = require('fs');


function mockConst(src, name) {
  const m = new RegExp('\\nconst ' + name + ' = ([\\s\\S]*?);\\n').exec(src);
  if (!m) throw new Error('flows.js has no const ' + name);
  return Function('return (' + m[1] + ')')();
}

// The repo rules page (d1..d3): data read off the mock's own constants so the product shows the same names, lines and counts.
function api(_state, mockDir) {
  const src = fs.readFileSync(path.join(mockDir, 'flows.js'), 'utf8');
  const m = { yaml: mockConst(src, 'RULES_YAML'), add: mockConst(src, 'NEW_RULE'), withheld: mockConst(src, 'WITHHELD'), direct: mockConst(src, 'DIRECT') };
  const text = m.yaml.join('\n') + '\n';
  const proposed = m.yaml.flatMap((t) => (t.endsWith('/issues/*/labels') ? [t, m.add] : [t])).join('\n') + '\n';
  const NOTE = 'Added one entry under api.github.com. It was withheld 3 times this week. Used the proxy-rules skill.';
  const rules = {
    repo: 'acme/web', text, revision: 12, edited_by: 'a developer', edited_at: '2026-11-18T10:00:00+08:00', can_edit: true,
    seen: {
      days: 7,
      withheld: m.withheld.map(([request, host, n]) => ({ request, host, boxes: Number(n) })),
      dialled: m.direct.map(([host, n]) => ({ host, port: 443, count: Number(n) })),
    },
  };
  const at = '2026-11-22T14:49:00+08:00';
  const ask = {
    box: 'rules-ask-1', repo: 'acme/web', other_repos: [], branch: null, base: 'main', prompt: 'Let Boxes in this repo comment on pull requests.',
    issue: null, machine: 'my-laptop', workflow: null, title: 'Rules of acme/web', state: 'done', stale: false, line: '',
    last_event_at: at, updated_at: at, pr: null, needs: null, gate: null, failed: false, retryable: false, owner: null,
    timeline: [
      { id: 1, at, kind: 'message', class: 'you', text: 'Let Boxes in this repo comment on pull requests.' },
      { id: 2, at, kind: 'message', class: 'message', text: NOTE + '\n\n\`\`\`yaml\n' + proposed + '\`\`\`' },
    ],
    artifacts: [], run_again: null, plan_first: false, from_workflow: false,
  };
  const account = {
    email: 'an@example.com', csrf: 'parity',
    secrets: [{ name: 'claude', linked: true }, { name: 'github', linked: true }],
    machines: { nodes: [], at: 0 },
    connect: { mcp_url: null, server_url: 'https://moor.acme.example' },
    profile: null,
  };
  return {
    '/account/api': account,
    '/account/api/boxes': { boxes: [] },
    '/account/api/inbox': { items: [], unread: 0 },
    '/account/api/repos': { repos: ['acme/api', 'acme/ios', 'acme/portal', 'acme/web'] },
    '/account/api/repos/acme/web/rules': rules,
    '/account/api/rules/ask': { box: 'rules-ask-1' },
    '/account/api/box?box=rules-ask-1': ask,
    '/account/api/session': {},
  };
}

module.exports = { api };
