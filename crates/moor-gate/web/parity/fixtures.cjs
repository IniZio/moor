'use strict';
const path = require('path');

const SCENE_STATE = { yes: 'needs_you', stuck: 'needs_you', nocheck: 'needs_you', working: 'working', fixed: 'done', nothing: 'done', failed: 'done', passed: 'done', nothingpr: 'done' };

function iso(ms) {
  const d = new Date(ms + 8 * 3600e3);
  return d.toISOString().replace('Z', '+08:00').replace('.000', '');
}

async function fixtures(page, mockDir, hide, tz) {
  await page.goto('file://' + path.resolve(mockDir) + '/index.html#/tasks?who=all&settled=1');
  await page.addStyleTag({ content: hide });
  await page.waitForSelector('body[data-ready]');
  const seen = await page.evaluate(() => {
    const M = window.MOCK;
    const now = {};
    document.querySelectorAll('a.row[data-task]').forEach((a) => { now[a.dataset.task] = a.querySelector('.rnow').textContent; });
    return M.tasks.concat(M.settled).map((t) => ({ id: t.id, kind: t.kind, repos: t.repos, key: t.key || null, pr: t.pr || null, title: t.title, scene: t.scene, owner: t.owner || null, settled: !!t.settled, at: t.at.getTime(), line: now[t.id] || t.line || '' }));
  });
  const row = (t) => ({
    box: 'mock-' + t.id,
    repo: t.repos[0],
    other_repos: t.repos.slice(1),
    branch: null, base: 'main', prompt: t.title,
    issue: t.key, machine: null, workflow: null,
    title: t.title,
    state: t.settled ? 'closed' : SCENE_STATE[t.scene],
    stale: false,
    line: t.line,
    last_event_at: iso(t.at), updated_at: iso(t.at),
    pr: t.kind === 'pr' && t.pr ? { repo: t.repos[0], number: t.pr, url: 'https://github.com/' + t.repos[0] + '/pull/' + t.pr } : null,
    needs: null, gate: null, failed: false, retryable: false,
    owner: t.owner,
  });
  const all = seen.map(row);
  const mine = all.filter((r) => !r.owner);
  const account = {
    email: 'tester@example.com', csrf: 'parity',
    secrets: [{ name: 'claude', linked: true }, { name: 'github', linked: true }],
    machines: { nodes: [], at: 0 },
    connect: { mcp_url: null, server_url: 'https://moor.acme.example' },
    profile: null,
  };
  return {
    seen,
    api: {
      '/account/api': account,
      '/account/api/boxes': { boxes: mine },
      '/account/api/boxes?who=everyone': { boxes: all },
      '/account/api/inbox': { items: [], unread: 0 },
      '/account/api/repos': { repos: [...new Set(seen.flatMap((t) => t.repos))].sort() },
      '/account/api/session': {},
    },
  };
}

module.exports = { fixtures };
