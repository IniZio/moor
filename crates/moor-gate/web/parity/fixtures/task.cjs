'use strict';
// Task screens (a1-a4, b2, b3, c2, c3, h1-h4): API answers that give the product the mock's data (names, repos, times, counts).
// PICS are the mock's own card pictures, shot off the mock and served on the product's evidence route: [name, mock screen, selector].
const TZ = '+08:00';
const at = (d, hm) => '2026-11-' + d + 'T' + hm + ':00' + TZ;
const acct = (email) => ({ email, csrf: 'audit', secrets: [{ name: 'claude', linked: true }, { name: 'github', linked: true }], machines: { nodes: [], at: 0 }, connect: { mcp_url: null, server_url: 'https://moor.acme.example' }, profile: null });
const row = (o) => ({ box: o.box, repo: o.repo, other_repos: [], branch: o.branch ?? null, base: 'main', prompt: o.prompt ?? o.title, issue: null, machine: null, workflow: o.workflow ?? null, title: o.title, state: o.state, stale: false, line: o.line ?? '', last_event_at: o.at, updated_at: o.at, pr: o.pr ?? null, needs: o.needs ?? null, gate: null, failed: o.state === 'failed', retryable: false, owner: null, task_at: o.task_at ?? null });
const HOME = [
  row({ box: 'b-api', repo: 'acme/api', title: 'Daily dependency check', state: 'needs_you', line: 'needs you: one upgrade breaks 2 tests', at: at(24, '09:00'), workflow: { name: 'Daily dependency check', pull: null } }),
  row({ box: 'b-book', repo: 'acme/clinic-prototype', title: 'Booking confirmation screen', state: 'working', line: 'writing the stories', at: at(24, '09:47') }),
  row({ box: 'b-web', repo: 'acme/web', title: 'Daily dependency check', state: 'done', line: '3 pull requests', at: at(24, '09:00'), workflow: { name: 'Daily dependency check', pull: null } }),
  row({ box: 'b-doc', repo: 'acme/clinic-prototype', title: 'Doctor list, filter chips', state: 'done', line: 'pull request #11 opened', at: at(23, '16:48') }),
];
let n = 0;
const you = (t, text) => ({ id: ++n, at: t, kind: 'message', class: 'you', text });
const say = (t, text) => ({ id: ++n, at: t, kind: 'message', class: 'message', text });
const tool = (t, name, title, group) => ({ id: ++n, key: 'k' + n, at: t, kind: 'tool', class: 'step', text: title, tool: { name, title, group: group ?? null } });
const reads = (t, k) => Array.from({ length: k }, (_, i) => tool(t, 'Read', 'src/file' + i + '.ts', 'read'));
const cmds = (t, k) => Array.from({ length: k }, (_, i) => tool(t, 'Bash', 'pnpm test ' + i));
const moor = (t, text) => ({ id: ++n, at: t, kind: 'moor', class: 'moor', text });
const ev = (name, t, image, v) => ({ name, version: v ?? 1, kind: 'evidence', content_type: image ? 'image/png' : 'text/markdown', size: 2048, created_at: t, image, href: '/account/api/box/evidence/' + encodeURIComponent(name) });
const PR = (repo, num) => ({ repo, number: num, url: 'https://github.com/' + repo + '/pull/' + num });
const out = (title, url, t) => ({ kind: 'pull request', url, title, first_at: Date.parse(t), last_at: Date.parse(t) });
const detail = (r, x) => ({ ...r, timeline: x.timeline, artifacts: x.artifacts ?? [], run_again: null, plan_first: false, from_workflow: !!r.workflow, task_boxes: x.task_boxes ?? [], outputs: x.outputs ?? [] });
const BRIEF = 'A date picker for the booking screen, in Storybook. Three states: default, selected, unavailable. Then send it for review.';
const BOX = 'acme/clinic-prototype';
function place(step) {
  n = 0;
  const tl = [you(at(24, '10:58'), 'Where is the booking fee calculated?'), ...reads(at(24, '10:59'), 6), say(at(24, '10:59'), 'In src/booking/fee.ts, calculateFee(). It charges 2% of the booking when the booking is above 500. confirmBooking() in src/booking/confirm.ts calls it.')];
  const boxes = [];
  if (step >= 2) {
    tl.push(you(at(24, '11:04'), 'Make it 3% above 500 and show me the booking screen with a 600 booking.'), say(at(24, '11:04'), 'That means running the app, so I started a Box.'), ...cmds(at(24, '11:06'), 14), say(at(24, '11:09'), 'Changed the rate in calculateFee() and ran the booking screen.'));
    boxes.push({ id: 1, name: 'fee-box-1', repo: BOX, started_at: at(24, '11:05'), ready_ms: 38000, state: step === 2 ? 'running' : 'closed', closed_at: step === 2 ? null : at(24, '11:11'), artifacts: step >= 3 ? [ev('Booking screen, fee line', at(24, '11:09'), true)] : [] });
  }
  if (step >= 3) tl.push(say(at(24, '11:10'), 'The change is in the pull request ' + PR(BOX, 15).url + '. I closed the Box.'));
  if (step >= 4) {
    tl.push(you(at(25, '09:12'), 'Round the fee to whole cents.'), say(at(25, '09:12'), 'Starting a Box again; the first one is gone, the branch has the work.'));
    boxes.push({ id: 2, name: 'fee-box-2', repo: BOX, started_at: at(25, '09:13'), ready_ms: 9000, state: 'running', closed_at: null, artifacts: [] });
  }
  const r = row({ box: 'h' + step, repo: BOX, title: 'Where is the booking fee calculated?', state: step === 1 || step === 3 ? 'done' : 'working', at: tl[tl.length - 1].at, task_at: at(24, '10:58'), pr: step >= 3 ? PR(BOX, 15) : null });
  return detail(r, { timeline: tl, task_boxes: boxes });
}
function design(step) {
  n = 0;
  const tl = [you(at(24, '10:02'), BRIEF), ...cmds(at(24, '10:03'), 9), say(at(24, '10:05'), 'Read the booking screen and the existing inputs.')];
  const arts = [ev('Date picker spec', at(24, '10:05'), false)];
  let needs = null, state = 'needs_you', pr = null;
  if (step === 3) needs = { id: 'q1', kind: 'question', question: 'Build it like this?', detail: null, picks: [{ n: 1, label: 'Build it', role: 'choice' }, { n: 2, label: 'Change the spec', role: 'choice' }], other: 3, approve: null, plan: false, gate: null };
  else {
    state = 'done';
    tl.push(you(at(24, '10:09'), 'Build it'), ...cmds(at(24, '10:12'), 31), you(at(24, '10:21'), 'Tighter rows, and the selected day should use the primary colour.'), say(at(24, '10:24'), 'Done. Row gap now uses space.2.'), say(at(24, '10:26'), 'The pull request is open and checks pass. Nothing is waiting on you. ' + PR(BOX, 14).url));
    arts.push(ev('Date picker stories', at(24, '10:18'), true, 1), ev('Date picker stories', at(24, '10:24'), true, 2));
    pr = PR(BOX, 14);
  }
  const r = row({ box: 'a' + step, repo: BOX, title: 'A date picker for the booking screen', state, at: tl[tl.length - 1].at, task_at: at(24, '10:02'), needs, pr });
  return detail(r, { timeline: tl, task_boxes: [{ id: 1, name: 'dp-box', repo: BOX, started_at: at(24, '10:03'), ready_ms: 21000, state: step === 3 ? 'running' : 'closed', closed_at: step === 3 ? null : at(24, '10:26'), artifacts: arts }] });
}
function deps(after) {
  n = 0;
  const tl = [{ ...you(at(24, '09:00'), 'Check every package for upgrades and security advisories. Open one pull request per safe upgrade; ask before a major one.'), kind: 'steered' }, ...cmds(at(24, '09:02'), 47), say(at(24, '09:20'), '5 packages have upgrades. 1 has a security advisory.')];
  if (after) tl.push(you(at(24, '09:41'), 'Skip it this time'), say(at(24, '09:42'), 'Left lodash at 4. Noted for tomorrow’s run.'));
  const needs = after ? null : { id: 'q2', kind: 'question', question: 'lodash 4 → 5 breaks 2 tests that pass on main. Open its pull request anyway?', detail: null, picks: [{ n: 1, label: 'Open it, marked failing', role: 'choice' }, { n: 2, label: 'Skip it this time', role: 'choice' }], other: 3, approve: null, plan: false, gate: null };
  const r = row({ box: after ? 'b3' : 'b2', repo: 'acme/api', branch: 'moor/deps-24-nov', title: 'Daily dependency check, 24 Nov', state: after ? 'done' : 'needs_you', line: after ? 'done' : 'needs you: one upgrade breaks 2 tests', at: tl[tl.length - 1].at, workflow: { name: 'Daily dependency check', pull: null }, needs });
  const arts = [ev('Upgrade report', at(24, '09:20'), true, 1), ev('Tests: branch against main', at(24, '09:20'), true)];
  if (after) arts.push(ev('Upgrade report', at(24, '09:42'), true, 2));
  return detail(r, { timeline: tl, artifacts: arts, outputs: [out('Pull request #212', PR('acme/api', 212).url, at(24, '09:20')), out('Pull request #213', PR('acme/api', 213).url, at(24, '09:20'))] });
}
function evidence(ok) {
  n = 0;
  const e = ok ? { repo: 'acme/web', num: 482, sha: 'a41c9e2', pr: 'Fix date picker overflow on small screens', k: 18, first: 'Ran the check and the booking screen at three widths.', last: 'Evidence is on the pull request. Nothing is waiting on you.', t: '14:03' }
    : { repo: 'acme/api', num: 96, sha: 'c09d77a', pr: 'Rate-limit the export endpoint', k: 4, first: 'Ran the check. It failed, so I stopped there.', last: 'Evidence is on the pull request, marked failed. A new push will run this again.', t: '13:20' };
  const [t0, t1] = ok ? ['14:01', '14:02'] : ['13:18', '13:19'];
  const tl = [...cmds(at(24, t0), e.k), say(at(24, t1), e.first), say(at(24, e.t), e.last)];
  const r = row({ box: ok ? 'c2' : 'c3', repo: e.repo, branch: 'moor/evidence-' + e.num, title: 'Evidence for #' + e.num + ' · ' + e.sha, state: ok ? 'done' : 'failed', line: ok ? 'evidence posted · checks pass' : 'failed: check did not pass', at: at(24, e.t), workflow: { name: 'Evidence on every pull request', pull: e.repo + '#' + e.num }, pr: PR(e.repo, e.num) });
  const arts = [ev('Evidence', at(24, t1), false)];
  if (ok) arts.push(ev('Booking screen, before and after', at(24, t1), true));
  return detail(r, { timeline: tl, artifacts: arts, outputs: [out('Comment on pull request #' + e.num, PR(e.repo, e.num).url + '#issuecomment-1', at(24, t1)), out('Status moor/evidence', PR(e.repo, e.num).url + '/checks', at(24, t1))] });
}
const DETAILS = { a3: design(3), a4: design(4), b2: deps(false), b3: deps(true), c2: evidence(true), c3: evidence(false), h1: place(1), h2: place(2), h3: place(3), h4: place(4) };
function api(state) {
  const { flow, DEPS, EVID } = require('./automation.cjs');
  const email = /^[bc]/.test(state.key) ? 'ana@acme.example' : 'maya@acme.example';
  const inbox = state.inbox ? [{ id: 1, box: 'b-api', repo: 'acme/api', kind: 'needs_input', state: 'unread', title: 'Daily dependency check', text: 'needs you: one upgrade breaks 2 tests', at: at(24, '09:00') }] : [];
  const a = { '/account/api': acct(email), '/account/api/boxes': { boxes: HOME }, '/account/api/boxes?who=everyone': { boxes: HOME }, '/account/api/inbox': { items: inbox, unread: inbox.length }, '/account/api/repos': { repos: ['acme/api', 'acme/clinic-prototype', 'acme/ios', 'acme/portal', 'acme/web'] }, '/account/api/session': {} };
  a['/account/api/workflows/Daily%20dependency%20check'] = flow(DEPS);
  a['/account/api/workflows/Evidence%20on%20every%20pull%20request'] = flow(EVID);
  for (const [k, d] of Object.entries(DETAILS)) {
    const q = '?box=' + encodeURIComponent(d.box);
    a['/account/api/box' + q] = d;
    a['/account/api/box/pr' + q] = { branch: d.branch ?? 'moor/x', repos: [{ repo: d.repo, state: d.pr ? 'open' : 'none', number: d.pr ? d.pr.number : null, checks: d.pr ? 'passing' : null, url: d.pr ? d.pr.url : null }] };
    a['/account/api/box/changes' + q] = { live: false, why: null, at: d.updated_at, repos: [] };
  }
  for (const [name, repo, num] of [['dp-box', BOX, 14], ['fee-box-1', BOX, 15], ['fee-box-2', BOX, 15]]) a['/account/api/box/pr?box=' + name] = { branch: 'moor/' + name, repos: [{ repo, state: 'open', number: num, checks: 'passing', url: PR(repo, num).url }] };
  return a;
}
const PICS = [['Date picker stories', '#a4', '#art-stories .apic'], ['Upgrade report', '#b2', '#art-report-1 .apic'], ['Tests: branch against main', '#b2', '.acard:nth-of-type(2) .apic'], ['Booking screen, fee line', '#h3', '.acard .apic'], ['Booking screen, before and after', '#c2', '.acard:has(.pks) .apic']];
module.exports = { api, PICS };
