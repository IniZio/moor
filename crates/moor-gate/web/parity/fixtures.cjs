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
  // The pull request each issue task opened: app.js keeps this map (PRS) inside its closure.
  const prs = { 41: 12, 43: 34, 44: 14, 40: 13, 39: 7, 30: 9, 31: 31, 32: 8, 33: 5, 70: 15, 71: 41, 72: 9 };
  const { tasks: seen, github } = await page.evaluate((prs) => {
    const M = window.MOCK;
    const now = {};
    document.querySelectorAll('a.row[data-task]').forEach((a) => { now[a.dataset.task] = a.querySelector('.rnow').textContent; });
    const tasks = M.tasks.concat(M.settled).map((t) => ({ id: t.id, kind: t.kind, repos: t.repos, key: t.key || null, pr: (t.kind === 'pr' ? t.pr : prs[t.id]) || null, title: t.title, scene: t.scene, owner: t.owner || null, settled: !!t.settled, at: t.at.getTime(), line: now[t.id] || t.line || '' }));
    return { tasks, github: M.github };
  }, prs);
  // The mock's own timeline rows for the four tasks the task page states open, read off its task page.
  const lines = {};
  for (const id of [41, 43, 44, 40]) {
    const scene = seen.find((t) => t.id === id).scene;
    await page.goto('file://' + path.resolve(mockDir) + '/index.html#/task/' + id + '?scene=' + scene);
    await page.waitForSelector('.tl');
    lines[id] = await page.evaluate(() =>
      [...document.querySelectorAll('.tl > li')].map((li) =>
        li.classList.contains('tl-fold')
          ? ['fold', li.querySelector('summary').textContent, [...li.querySelectorAll('ul li')].map((x) => x.textContent)]
          : [li.className.replace('tl-', ''), li.querySelector('.tl-text').textContent, li.querySelector('.tl-when').textContent],
      ),
    );
  }
  const clock = (at, hhmm) => {
    const [h, m] = hhmm.split(':').map(Number);
    const d = new Date(at + 8 * 3600e3);
    let ms = Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate(), h, m) - 8 * 3600e3;
    if (ms > at + 60e3) ms -= 86400e3;
    return ms;
  };
  const entries = (id) => {
    const items = lines[id];
    if (!items) return null;
    const task = seen.find((t) => t.id === id);
    let n = 0;
    const out = [];
    items.forEach((it, i) => {
      if (it[0] === 'fold') {
        const next = items.slice(i).find((x) => x[0] !== 'fold') || items[i - 1];
        const at = iso(clock(task.at, next[2]));
        const reads = (/^Read (\d+) files/.exec(it[1]) || [])[1];
        if (/^Set up/.test(it[1])) it[2].forEach((m) => out.push({ id: ++n, at, kind: 'setup', class: 'routine', text: m }));
        else for (let k = 0; k < Number(reads || 0); k++) out.push({ id: ++n, key: 'tool-' + id + '-' + n, at, kind: 'tool', class: 'step', text: 'Read', tool: { name: 'Read', title: 'Read file', group: 'read' } });
        return;
      }
      const cls = { you: 'you', agent: 'message', moor: 'moor', them: 'you' }[it[0]];
      out.push({ id: ++n, at: iso(clock(task.at, it[2])), kind: it[0] === 'moor' ? 'moor' : 'message', class: cls, text: it[1] });
    });
    return out;
  };
  const row = (t) => ({
    box: 'mock-' + t.id,
    repo: t.repos[0],
    other_repos: t.repos.slice(1),
    branch: null, base: 'main', prompt: t.title,
    issue: t.key, machine: null, workflow: t.kind === 'pr' ? { name: 'evidence', pull: t.repos[0] + '#' + t.pr } : null,
    title: t.title,
    state: t.settled ? 'closed' : SCENE_STATE[t.scene],
    stale: false,
    line: t.line,
    last_event_at: iso(t.at), updated_at: iso(t.at),
    pr: t.pr ? { repo: t.repos[0], number: t.pr, url: 'https://github.com/' + t.repos[0] + '/pull/' + t.pr } : null,
    needs: null, gate: null, failed: false, retryable: false,
    owner: t.owner,
  });
  const GATES = {
    41: { gates: ['proof', 'tests', 'review'], status: { state: 'open', gate: 'review', try_n: 1, next: 'ask' }, wait: { gate: 'review', kind: 'approve', seq: 1, handed: ['before.png', 'after.png'], note: 'The avatar sat in a 48px box that hid its overflow, so tall pictures lost their top. The box is square now and the picture covers it.' } },
    43: { gates: ['tests', 'review'], status: { state: 'stuck', gate: 'tests', why: 'exited with code 1' }, wait: { gate: 'tests', kind: 'stuck', seq: 1, try_n: 2, why: 'exited with code 1', command: 'make test', says: 'all tests pass', tail: '--- FAIL: TestRefundOldOrder (0.02s)\n    status = 409, want 200' } },
    57: { gates: ['evidence', 'tests', 'comment'], status: { state: 'stuck', gate: 'tests', why: 'could not run', unanswered: { check: 'npm run e2e', command: 'npm run e2e', says: null, reason: { kind: 'lost' } } }, wait: { gate: 'tests', kind: 'stuck', seq: 1, try_n: 2, command: 'npm run e2e', unanswered: { check: 'npm run e2e', command: 'npm run e2e', says: null, reason: { kind: 'lost' } } } },
    44: { gates: ['proof', 'tests', 'review'], status: { state: 'open', gate: 'tests', try_n: 1, next: 'agent' } },
    58: { gates: ['evidence', 'tests', 'comment'], status: { state: 'open', gate: 'tests', try_n: 1, next: 'run_checks' } },
    40: { gates: ['proof', 'tests', 'review'], status: { state: 'passed', outcome: { name: 'fixed', tone: 'good', says: null } } },
    56: { gates: ['evidence', 'tests', 'comment'], status: { state: 'passed', outcome: { name: 'check failed', tone: 'bad', says: null } } },
    55: { gates: ['evidence', 'tests', 'comment'], status: { state: 'passed', outcome: { name: 'check passed', tone: 'good', says: null } } },
    54: { gates: ['evidence', 'tests', 'comment'], gates_passed: 0, status: { state: 'passed', outcome: { name: 'nothing to do', tone: 'plain', says: null } } },
    39: { gates: ['proof', 'tests', 'review'], gates_passed: 0, status: { state: 'passed', outcome: { name: 'nothing to fix', tone: 'plain', says: null } } },
  };
  const withGate = (r, t) => {
    const g = GATES[t.id];
    if (!g) return r;
    r.gate = { gates: g.gates, status: g.status };
    // The line the gate writes for a gated row, so a page that shows the mock's words has made them itself.
    const s = g.status;
    if (s.state === 'open') r.line = s.next === 'ask' ? `gate ${s.gate}: waiting for your approval` : 'reading the code';
    if (s.state === 'stuck') r.line = `gate ${s.gate} is stuck: ${s.why}`;
    if (g.gates_passed !== undefined) r.gate.gates_passed = g.gates_passed;
    if (g.wait) r.needs = { id: 'ask-' + t.id, kind: g.wait.kind, question: t.line, detail: null, picks: [], other: null, approve: null, plan: false, gate: g.wait };
    return r;
  };
  const all = seen.map((t) => withGate(row(t), t));
  const detail = (r) => ({
    ...r,
    branch: 'moor/' + r.box.slice(5),
    machine: 'my-laptop',
    timeline: entries(Number(r.box.slice(5))) || [
      { id: 1, at: r.updated_at, kind: 'message', class: 'you', text: r.prompt },
      { id: 2, at: r.updated_at, kind: 'message', class: 'message', text: 'I found where this happens. I will fix it there and add a test.' },
    ],
    artifacts: (r.needs?.gate?.handed ?? (lines[r.box.slice(5)] ? ['before.png', 'after.png'] : [])).map((name, i) => ({ name, version: 1, kind: 'evidence', content_type: 'image/png', size: 2048, created_at: r.updated_at, image: true, href: '/account/api/box/evidence/' + name })),
    run_again: null, plan_first: false, from_workflow: !!r.pr,
  });
  const boxApi = {};
  for (const r of all) {
    const q = '?box=' + encodeURIComponent(r.box);
    boxApi['/account/api/box' + q] = detail(r);
    boxApi['/account/api/box/pr' + q] = { branch: 'moor/' + r.box.slice(5), repos: [{ repo: r.repo, state: r.pr ? 'open' : 'none', number: r.pr ? r.pr.number : null, checks: null, url: r.pr ? r.pr.url : null }] };
    boxApi['/account/api/box/changes' + q] = { live: false, why: null, at: r.updated_at, repos: [] };
  }
  const inbox = all.filter((r) => r.state === 'needs_you' && !r.owner).map((r, i) => ({ id: i + 1, box: r.box, repo: r.repo, kind: 'needs_input', state: 'unread', title: r.title, text: r.line, at: r.updated_at }));
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
    png: 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAAAAAA6fptVAAAACklEQVR4nGPY0QAAAkABIGVN9fkAAAAASUVORK5CYII=',
    api: {
      '/account/api': account,
      '/account/api/boxes': { boxes: mine },
      '/account/api/boxes?who=everyone': { boxes: all },
      '/account/api/inbox': { items: inbox, unread: inbox.length },
      ...boxApi,
      '/account/api/repos': { repos: [...github].sort() },
      '/account/api/session': {},
    },
  };
}

module.exports = { fixtures };
