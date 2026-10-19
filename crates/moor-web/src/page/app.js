'use strict';
const $ = (id) => document.getElementById(id);
const nodes = new Map();
const jobOut = new Map();
let treeTimer = null;
let pending = null;

function h(tag, text, cls) {
  const e = document.createElement(tag);
  if (text != null) e.textContent = text;
  if (cls) e.className = cls;
  return e;
}

function control(tag, label, attrs) {
  const e = document.createElement(tag);
  e.setAttribute('aria-label', label);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (k === 'text') e.textContent = v;
    else if (k === 'class') e.className = v;
    else e.setAttribute(k, v);
  }
  return e;
}

async function api(path, body) {
  const opts = { credentials: 'same-origin' };
  if (body !== undefined) {
    opts.method = 'POST';
    opts.headers = { 'Content-Type': 'application/json', 'X-Moor-Csrf': '1' };
    opts.body = JSON.stringify(body);
  }
  const res = await fetch(path, opts);
  const text = await res.text();
  let json = null;
  try { json = JSON.parse(text); } catch (_) { /* plain text error */ }
  return { ok: res.ok, status: res.status, text, json };
}

function show(out, text, kind) {
  out.textContent = text;
  out.className = 'msg' + (kind ? ' ' + kind : '');
}

function append(out, text, kind) {
  const line = h('div', text, 'msg' + (kind ? ' ' + kind : ''));
  out.appendChild(line);
  return line;
}

function fmtTime(ts) {
  if (ts == null) return '';
  const d = new Date(typeof ts === 'number' && ts < 1e12 ? ts * 1000 : ts);
  return isNaN(d) ? String(ts) : d.toLocaleTimeString();
}

function shortKey(k) {
  return k.length > 8 ? k.slice(0, 8) + '…' : k;
}

function prUrl(task) {
  if (!task || !task.repo_url || !task.branch) return null;
  const m = /^(?:https:\/\/github\.com\/|git@github\.com:)([^/\s]+)\/([^/\s]+?)(?:\.git)?\/?$/.exec(task.repo_url.trim());
  if (!m) return null;
  return 'https://github.com/' + m[1] + '/' + m[2] + '/pulls?q=' + encodeURIComponent('head:' + task.branch);
}

async function act(path, body, out) {
  show(out, 'working…');
  try {
    const r = await api(path, body);
    if (!r.ok) { show(out, r.status + ': ' + r.text, 'bad'); return null; }
    const env = r.json;
    if (env && env.class && env.class !== 'ok') {
      show(out, env.class + ': ' + ((env.error && env.error.message) || 'no detail'), 'bad');
      const hints = env.error && Array.isArray(env.error.hints) ? env.error.hints : [];
      for (const hint of hints) append(out, String(hint), 'bad');
    } else {
      show(out, 'ok');
    }
    scheduleTree();
    return env;
  } catch (e) {
    show(out, 'request failed: ' + e.message, 'bad');
    return null;
  }
}

function confirmThen(label, message, fn) {
  const dlg = $('confirm');
  dlg.setAttribute('aria-label', label);
  $('confirm-text').textContent = message;
  pending = fn;
  dlg.returnValue = '';
  dlg.showModal();
  $('confirm-no').focus();
}

function followJob(jobId, out, onDone) {
  const es = new EventSource('/jobs/' + encodeURIComponent(jobId) + '/events');
  let finished = false;
  const finish = () => { finished = true; es.close(); scheduleTree(); if (onDone) onDone(); };
  es.addEventListener('progress', (e) => {
    let t = e.data;
    try { t = JSON.parse(e.data); } catch (_) { /* raw text */ }
    append(out, typeof t === 'string' ? t : JSON.stringify(t));
  });
  es.addEventListener('done', (e) => {
    let r = null;
    try { r = JSON.parse(e.data); } catch (_) { /* ignore */ }
    const fs = r && r.start && r.start.failed_step;
    append(out, fs ? 'done, but setup step failed: ' + fs : 'done', fs ? 'warn' : null);
    finish();
  });
  es.addEventListener('failed', (e) => {
    let f = { step: 'unknown', message: e.data };
    try { f = JSON.parse(e.data); } catch (_) { /* raw text */ }
    append(out, 'failed at ' + f.step + ': ' + f.message, 'bad');
    if (f.step === 'close-children') {
      append(out, 'Partial failure: some worker Boxes may still be open. Close them individually.', 'bad');
    }
    finish();
  });
  es.onerror = () => {
    if (!finished) { append(out, 'job stream lost', 'bad'); finish(); }
  };
}

async function startJob(path, body, out, onDone) {
  show(out, 'starting…');
  try {
    const r = await api(path, body);
    if (!r.ok || !r.json || !r.json.job) { show(out, r.status + ': ' + r.text, 'bad'); return; }
    out.textContent = '';
    followJob(r.json.job, out, onDone);
  } catch (e) {
    show(out, 'request failed: ' + e.message, 'bad');
  }
}

/* ---- tree ---- */

function makeNode(n) {
  const li = h('li');
  li.setAttribute('role', 'treeitem');
  li.dataset.sid = String(n.session);
  const card = h('div', null, 'card');
  const r = { li, card, box: n.box, request: n.request, qsig: null, proofKey: null };
  const sid = n.session;

  const head = h('div', null, 'row');
  r.toggle = control('button', 'Toggle children of session ' + sid, { type: 'button', text: '▾' });
  r.toggle.addEventListener('click', () => {
    r.kids.hidden = !r.kids.hidden;
    r.toggle.textContent = r.kids.hidden ? '▸' : '▾';
    li.setAttribute('aria-expanded', String(!r.kids.hidden));
  });
  r.title = h('strong');
  r.badge = h('span', null, 'badge');
  r.meta = h('span', null, 'muted');
  r.pr = h('a', 'Open PR search');
  r.pr.target = '_blank';
  r.pr.rel = 'noopener noreferrer';
  r.pr.setAttribute('aria-label', 'Pull requests for session ' + sid);
  head.append(r.toggle, r.title, r.badge, r.meta, r.pr);
  r.goal = h('div', null, 'muted');
  card.append(head, r.goal);

  r.ask = h('div', null, 'ask');
  r.askText = h('div');
  r.askOpts = h('div', null, 'row');
  const arow = h('div', null, 'row');
  r.askInput = control('input', 'Answer text for session ' + sid, { type: 'text', placeholder: 'type an answer' });
  const answerSend = control('button', 'Send answer for session ' + sid, { type: 'button', text: 'Send' });
  r.askOut = h('div', null, 'msg');
  const sendAnswer = (body) => act('/answer', Object.assign({ task: r.box }, body), r.askOut);
  answerSend.addEventListener('click', () => {
    const text = r.askInput.value.trim();
    if (text) sendAnswer({ text }).then((env) => { if (env && env.class === 'ok') r.askInput.value = ''; });
  });
  r.askInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') answerSend.click(); });
  arow.append(r.askInput, answerSend);
  r.ask.append(r.askText, r.askOpts, arow, r.askOut);
  r.ask.hidden = true;
  r.sendAnswer = sendAnswer;
  card.appendChild(r.ask);

  if (n.box) {
    const row = h('div', null, 'row');
    const steer = control('input', 'Steer text for session ' + sid, { type: 'text', placeholder: 'steer this worker' });
    const sb = control('button', 'Steer session ' + sid, { type: 'button', text: 'Steer' });
    const eb = control('button', 'End session ' + sid, { type: 'button', text: 'End session' });
    const cb = control('button', 'Close box ' + n.box, { type: 'button', text: 'Close box', class: 'danger' });
    r.actOut = h('div', null, 'msg');
    sb.addEventListener('click', async () => {
      const text = steer.value.trim();
      if (!text) return;
      const env = await act('/steer', { task: r.box, text }, r.actOut);
      if (env && env.class === 'ok') steer.value = '';
    });
    steer.addEventListener('keydown', (e) => { if (e.key === 'Enter') sb.click(); });
    eb.addEventListener('click', () => act('/end-session', { task: r.box }, r.actOut));
    cb.addEventListener('click', () => confirmThen('Close box ' + n.box, 'Close Box ' + n.box + '?',
      () => act('/close', { task: r.box }, r.actOut)));
    row.append(steer, sb, eb, cb);
    card.append(row, r.actOut);

    r.proofs = h('details');
    r.proofs.appendChild(h('summary', 'Proof files'));
    r.proofList = h('div', null, 'msg');
    r.proofs.appendChild(r.proofList);
    r.proofs.addEventListener('toggle', () => { if (r.proofs.open) loadProofs(r); });
    card.appendChild(r.proofs);
  }

  if (n.request) {
    const key = n.request.key;
    r.reqBox = h('div');
    const row = h('div', null, 'row');
    const task = control('input', 'New worker task for request ' + key, { type: 'text', placeholder: 'worker task' });
    const goal = control('input', 'New worker goal for request ' + key, { type: 'text', placeholder: 'worker goal' });
    const add = control('button', 'Add worker to request ' + key, { type: 'button', text: 'Add worker' });
    const end = control('button', 'End request ' + key, { type: 'button', text: 'End request', class: 'danger' });
    r.jobs = h('div', null, 'jobs');
    jobOut.set(key, r.jobs);
    add.addEventListener('click', () => {
      if (!task.value.trim() || !goal.value.trim()) { show(r.jobs, 'Worker needs a task name and a goal.', 'bad'); return; }
      startJob('/requests/' + encodeURIComponent(key) + '/workers', { task: task.value.trim(), goal: goal.value.trim() }, r.jobs,
        () => { task.value = ''; goal.value = ''; });
    });
    end.addEventListener('click', () => confirmThen('End request ' + key,
      'End request? This closes every worker Box and ends the request.',
      () => startJob('/requests/' + encodeURIComponent(key) + '/end', {}, r.jobs)));
    row.append(task, goal, add, end);
    r.reqBox.append(row, r.jobs);
    card.appendChild(r.reqBox);
    card.classList.add('root');
  }

  r.kids = h('ul');
  r.kids.setAttribute('role', 'group');
  li.append(card, r.kids);
  return r;
}

function updateNode(r, n) {
  const ended = n.state === 'gone' || !!(n.request && n.request.ended_at);
  const label = n.request ? 'Request ' + shortKey(n.request.key) + (n.request.repo ? ' · ' + n.request.repo : '')
    : (n.task && n.task.name ? n.task.name : (n.box || 'Session')) + ' (session ' + n.session + ')';
  r.title.textContent = label;
  const state = (n.agent && n.agent.state) || n.state || '';
  r.badge.textContent = state.replace('_', ' ');
  r.badge.className = 'badge s-' + state;
  const meta = [];
  if (n.box) meta.push('Box ' + n.box + (n.task && n.task.box_state ? ' (' + n.task.box_state + ')' : ''));
  if (n.task && n.task.branch) meta.push(n.task.branch);
  if (n.task && n.task.state) meta.push('task ' + n.task.state);
  if (ended) meta.push('ended');
  r.meta.textContent = meta.join(' · ');
  r.goal.textContent = (n.task && n.task.goal) || '';
  r.goal.hidden = !r.goal.textContent;
  const url = prUrl(n.task);
  r.pr.hidden = !url;
  if (url) r.pr.href = url;
  r.card.classList.toggle('ended', ended);
  if (r.reqBox) r.reqBox.hidden = ended;

  const q = n.agent && n.agent.question;
  const asking = !!q || (n.agent && n.agent.state === 'needs_input');
  r.ask.hidden = !asking;
  const sig = asking ? (q || '') + '\u0001' + ((n.agent && n.agent.options) || []).join('\u0001') : null;
  if (asking && sig !== r.qsig) {
    r.askText.textContent = q || 'The agent is waiting for input.';
    r.askOpts.textContent = '';
    ((n.agent && n.agent.options) || []).forEach((opt, i) => {
      const b = control('button', 'Answer option ' + (i + 1) + ' for session ' + n.session, { type: 'button', text: (i + 1) + '. ' + opt });
      b.addEventListener('click', () => r.sendAnswer({ option: i + 1 }));
      r.askOpts.appendChild(b);
    });
  }
  r.qsig = sig;
  r.asking = asking;

  const hasKids = n.children.length > 0;
  r.toggle.hidden = !hasKids;
  if (hasKids) r.li.setAttribute('aria-expanded', String(!r.kids.hidden));
  else r.li.removeAttribute('aria-expanded');
}

function syncList(ul, list) {
  const keep = new Set(list.map((n) => String(n.session)));
  for (const li of Array.from(ul.children)) {
    if (!keep.has(li.dataset.sid)) dropNode(li);
  }
  list.forEach((n, i) => {
    let r = nodes.get(n.session);
    if (!r) { r = makeNode(n); nodes.set(n.session, r); }
    updateNode(r, n);
    if (ul.children[i] !== r.li) ul.insertBefore(r.li, ul.children[i] || null);
    syncList(r.kids, n.children);
  });
}

function dropNode(li) {
  for (const sub of li.querySelectorAll('li[data-sid]')) nodes.delete(Number(sub.dataset.sid));
  nodes.delete(Number(li.dataset.sid));
  li.remove();
}

async function loadProofs(r) {
  if (!r.box || !r.proofs) return;
  try {
    const res = await api('/boxes/' + encodeURIComponent(r.box));
    if (!res.ok || !res.json) { r.proofList.textContent = 'could not load proofs: ' + res.text; return; }
    const { proofs = [], events = [] } = res.json;
    r.proofList.textContent = '';
    if (!proofs.length) r.proofList.appendChild(h('div', 'No proof files yet.', 'muted'));
    for (const p of proofs) r.proofList.appendChild(h('div', p.name + ' (' + p.size + ' bytes)'));
    for (const ev of events.filter((e) => e.kind === 'proof').slice(-10)) {
      r.proofList.appendChild(h('div', 'proof event ' + fmtTime(ev.ts) + ' ' + JSON.stringify(ev.payload), 'muted'));
    }
  } catch (e) {
    r.proofList.textContent = 'could not load proofs: ' + e.message;
  }
}

async function refreshTree() {
  try {
    const res = await api('/tree');
    if (!res.ok || !res.json) return;
    syncList($('tree'), res.json.roots);
    $('empty').hidden = res.json.roots.length > 0;
    let asks = 0;
    for (const r of nodes.values()) if (r.asking) asks++;
    $('asks').textContent = asks ? asks + ' open question' + (asks > 1 ? 's' : '') : '';
  } catch (_) { /* next event retries */ }
}

function scheduleTree() {
  clearTimeout(treeTimer);
  treeTimer = setTimeout(refreshTree, 300);
}

/* ---- live events ---- */

function feedLine(text) {
  const li = h('li', text);
  const feed = $('feed');
  feed.insertBefore(li, feed.firstChild);
  while (feed.children.length > 200) feed.lastChild.remove();
}

function connect() {
  const es = new EventSource('/events');
  const conn = $('conn');
  const setConn = (t, cls) => { conn.textContent = t; conn.className = 'badge ' + cls; };
  es.onopen = () => setConn('connected', 's-ok');
  es.addEventListener('error', (e) => {
    if (typeof e.data === 'string' && e.data) feedLine('stream error: ' + e.data);
    else setConn('reconnecting', 's-failed');
  });
  es.onmessage = (e) => {
    setConn('connected', 's-ok');
    let f;
    try { f = JSON.parse(e.data); } catch (_) { return; }
    const parts = [fmtTime(f.ts), f.kind || f.frame];
    if (f.box) parts.push('box ' + f.box);
    if (f.session != null) parts.push('session ' + f.session);
    feedLine(parts.join(' · '));
    if (f.kind === 'proof' && f.box) {
      for (const r of nodes.values()) if (r.box === f.box && r.proofs && r.proofs.open) loadProofs(r);
    }
    scheduleTree();
  };
}

/* ---- static controls ---- */

async function loadRepos() {
  const sel = $('new-repo');
  try {
    const res = await api('/repos');
    for (const r of (res.json && res.json.repos) || []) {
      const o = h('option', r.name);
      o.value = r.name;
      sel.appendChild(o);
    }
  } catch (_) { /* leave empty */ }
  if (!sel.children.length) {
    const o = h('option', 'no repos configured');
    o.value = '';
    sel.appendChild(o);
  }
  syncStart();
}

function syncStart() {
  $('new-start').disabled = !$('new-repo').value;
}

$('new-repo').addEventListener('change', syncStart);

$('new-start').addEventListener('click', () => {
  const out = $('new-out');
  const repo = $('new-repo').value;
  if (!repo) { show(out, 'No repo selected.', 'bad'); return; }
  const task = $('new-task').value.trim();
  const goal = $('new-goal').value.trim();
  if (!repo || !task || !goal) { show(out, 'Pick a repo and fill in task and goal.', 'bad'); return; }
  startJob('/requests', { repo, task, goal }, out, () => { $('new-task').value = ''; $('new-goal').value = ''; });
});

$('confirm-yes').addEventListener('click', () => $('confirm').close('yes'));
$('confirm-no').addEventListener('click', () => $('confirm').close('no'));
$('confirm').addEventListener('close', () => {
  const fn = pending;
  pending = null;
  if (fn && $('confirm').returnValue === 'yes') fn();
});

loadRepos();
refreshTree();
connect();
