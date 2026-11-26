'use strict';
// Automation screens (b1, c1 and their states): product data that matches the mock's AUTO, RUNS and EVRUNS in flows.js.
const HK = (s) => Date.parse(s + '+08:00');
const B1_PROMPT = 'Check this repo for dependency upgrades and open one pull request for each, never as a draft. Never merge a pull request, never dismiss a security alert, and never push to the default branch. Run the repo’s end-to-end tests on your branch and on main, and compare them test by test, because main already has known failures. If an upgrade breaks a test that passes on main, ask before you open its pull request. Say what you found in one short report.';
const C1_PROMPT = 'Check out the pull request and run the repo’s check. Run the change and take screenshots of what it touches. Post one comment on the pull request and set the moor/evidence status; never push, approve or merge. Say plainly what you did not run.';
const CHAT_SENTENCE = 'Also post the summary to the team chat channel #web-prs.';
const ST = { done: 'success', needs: 'waiting', failed: 'failure', settled: 'skipped', working: 'running' };
let id = 100;
const run = (st, label, at, pull, sha) => ({ id: ++id, due_at: HK(at), trigger: pull ? 'pull_request' : 'schedule', pull: pull || null, head_sha: sha || null, started_at: HK(at), duration_ms: null, status: ST[st], label, verdict: null, result: null, detail: null });
const B1_RUNS = [
  run('done', '3 pull requests · #214 #215 #216', '2026-11-24T09:00'), run('needs', 'needs you: one upgrade breaks 2 tests', '2026-11-24T09:00'), run('done', 'nothing to upgrade', '2026-11-24T09:00'), run('failed', 'failed: e2e stack did not start', '2026-11-24T09:00'),
  run('done', '2 pull requests · #207 #208', '2026-11-23T09:00'), run('done', 'nothing to upgrade', '2026-11-23T09:00'), run('done', 'nothing to upgrade', '2026-11-23T09:00'), run('done', '2 pull requests · #31 #32', '2026-11-23T09:00'),
];
const C1_RUNS = [
  run('done', 'evidence posted · checks pass', '2026-11-24T14:03', 'acme/web#482', 'a41c9e2d0b1c4e5f6a7b8c9d0e1f2a3b4c5d6e7f'),
  run('settled', 'replaced by a newer push', '2026-11-24T13:52', 'acme/web#482', '7be03d1aa0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5'),
  run('failed', 'failed: check did not pass', '2026-11-24T13:20', 'acme/api#96', 'c09d77a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7'),
  run('done', 'evidence posted', '2026-11-23T16:30', 'acme/web#479', '1f2a8c4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f'),
];
const row = (o) => ({ name: o.name, repos: o.repos, cron: o.cron, words: o.words, timezone: o.tz, enabled: true, on_pull_request: !!o.pr, next_due: o.cron ? HK('2026-11-25T09:00') : null, next_text: o.cron ? 'Wed 25 Nov, 09:00 (Asia/Hong_Kong)' : null, has_plan: false, plan_stale: false, last: { status: o.runs[0].status, label: o.runs[0].label, verdict: null, due_at: o.runs[0].due_at } });
const flow = (o) => ({ ...row(o), timeout_minutes: 60, prompt: o.prompt, plan: null, plan_making: null, gates: null, gates_by: null, outcomes: null, revision: o.rev, runs: o.runs, run_as: 'ana', may: { allow: { 'api.github.com': o.may } } });
const DEPS = { name: 'deps-check', repos: ['acme/web', 'acme/api', 'acme/ios', 'acme/portal'], cron: '0 9 * * 1-5', words: 'Every weekday at 09:00', tz: 'Asia/Hong_Kong', prompt: B1_PROMPT, rev: 7, runs: B1_RUNS, may: ['POST /repos/{repo}/pulls'] };
const EVID = { name: 'pr-evidence', repos: ['acme/web', 'acme/api'], cron: null, words: 'On every pull request and every push to it', tz: null, pr: true, prompt: C1_PROMPT, rev: 3, runs: C1_RUNS, may: ['POST /repos/{repo}/issues/*/comments', 'PATCH /repos/{repo}/issues/comments/*', 'POST /repos/{repo}/statuses/*'] };
const account = { email: 'ana@acme.example', csrf: 'parity', secrets: [{ name: 'claude', linked: true }, { name: 'github', linked: true }], machines: { nodes: [], at: 0 }, connect: { mcp_url: null, server_url: 'https://moor.acme.example' }, profile: null };
const apiFor = () => ({
  '/account/api': account,
  '/account/api/boxes': { boxes: [] },
  '/account/api/boxes?who=everyone': { boxes: [] },
  '/account/api/inbox': { items: [], unread: 0 },
  '/account/api/repos': { repos: ['acme/api', 'acme/ios', 'acme/portal', 'acme/web'] },
  '/account/api/session': {},
  '/account/api/workflows': { workflows: [row(DEPS), row(EVID)] },
  '/account/api/workflows/deps-check': flow(DEPS),
  '/account/api/workflows/pr-evidence': flow(EVID),
});
module.exports = { api: apiFor, flow, DEPS, EVID, CHAT_SENTENCE, C1_PROMPT };
