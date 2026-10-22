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

function prUrl(view) {
  if (!view || !view.repo_url || !view.branch) return null;
  const m = /^(?:https:\/\/github\.com\/|git@github\.com:)([^/\s]+)\/([^/\s]+?)(?:\.git)?\/?$/.exec(view.repo_url.trim());
  if (!m) return null;
  return 'https://github.com/' + m[1] + '/' + m[2] + '/pulls?q=' + encodeURIComponent('head:' + view.branch);
}

function fmtSize(n) {
  if (typeof n !== 'number') return '';
  if (n < 1024) return n + ' B';
  const units = ['KB', 'MB', 'GB'];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return v.toFixed(v < 10 ? 1 : 0) + ' ' + units[i];
}

async function loadArtifacts(box, list) {
  const base = '/boxes/' + encodeURIComponent(box) + '/artifacts/';
  try {
    const res = await api('/boxes/' + encodeURIComponent(box) + '/artifacts');
    if (!res.ok || !res.json) { show(list, res.status + ': ' + res.text, 'bad'); return; }
    const files = Array.isArray(res.json) ? res.json : res.json.artifacts || [];
    list.textContent = '';
    list.className = 'artifacts';
    if (!files.length) { list.appendChild(h('div', 'No artifacts yet.', 'muted')); return; }
    for (const f of files) {
      const url = base + encodeURIComponent(f.name) + '?v=' + encodeURIComponent(f.version);
      const link = h('a');
      link.href = url;
      link.target = '_blank';
      link.rel = 'noopener noreferrer';
      if (String(f.content_type).startsWith('image/')) {
        const img = h('img');
        img.src = url;
        img.alt = f.name;
        img.className = 'thumb';
        link.append(img);
      } else {
        link.textContent = f.name + ' (' + fmtSize(f.size) + ')';
      }
      list.appendChild(h('div')).appendChild(link);
    }
  } catch (e) {
    show(list, 'request failed: ' + e.message, 'bad');
  }
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
  const r = { li, card, box: n.name, request: n.request, qsig: null };
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
  const sendAnswer = (body) => act('/answer', Object.assign({ box: r.box }, body), r.askOut);
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

  if (n.name) {
    const row = h('div', null, 'row');
    const steer = control('input', 'Steer text for session ' + sid, { type: 'text', placeholder: 'steer this worker' });
    const sb = control('button', 'Steer session ' + sid, { type: 'button', text: 'Steer' });
    const eb = control('button', 'End session ' + sid, { type: 'button', text: 'End session' });
    const cb = control('button', 'Close box ' + n.name, { type: 'button', text: 'Close box', class: 'danger' });
    r.actOut = h('div', null, 'msg');
    sb.addEventListener('click', async () => {
      const text = steer.value.trim();
      if (!text) return;
      const env = await act('/steer', { box: r.box, text }, r.actOut);
      if (env && env.class === 'ok') steer.value = '';
    });
    steer.addEventListener('keydown', (e) => { if (e.key === 'Enter') sb.click(); });
    eb.addEventListener('click', () => act('/end-session', { box: r.box }, r.actOut));
    cb.addEventListener('click', () => confirmThen('Close box ' + n.name, 'Close Box ' + n.name + '?',
      () => act('/close', { box: r.box }, r.actOut)));
    row.append(steer, sb, eb, cb);
    card.append(row, r.actOut);

    const ab = control('button', 'Toggle artifacts of box ' + n.name, { type: 'button', text: 'Artifacts' });
    const list = h('div', null, 'artifacts');
    list.hidden = true;
    const reload = () => { if (!list.hidden) loadArtifacts(n.name, list); };
    ab.addEventListener('click', () => { list.hidden = !list.hidden; reload(); });
    r.reloadArtifacts = reload;
    const artRow = h('div', null, 'row');
    artRow.appendChild(ab);
    card.append(artRow, list);
  }

  if (n.request) {
    const key = n.request.key;
    r.reqBox = h('div');
    const row = h('div', null, 'row');
    const name = control('input', 'New worker Box for request ' + key, { type: 'text', placeholder: 'worker Box name' });
    const goal = control('input', 'New worker goal for request ' + key, { type: 'text', placeholder: 'worker goal' });
    const add = control('button', 'Add worker to request ' + key, { type: 'button', text: 'Add worker' });
    const end = control('button', 'End request ' + key, { type: 'button', text: 'End request', class: 'danger' });
    r.jobs = h('div', null, 'jobs');
    jobOut.set(key, r.jobs);
    add.addEventListener('click', () => {
      if (!name.value.trim() || !goal.value.trim()) { show(r.jobs, 'Worker needs a Box name and a goal.', 'bad'); return; }
      startJob('/requests/' + encodeURIComponent(key) + '/workers', { name: name.value.trim(), goal: goal.value.trim() }, r.jobs,
        () => { name.value = ''; goal.value = ''; });
    });
    end.addEventListener('click', () => confirmThen('End request ' + key,
      'End request? This closes every worker Box and ends the request.',
      () => startJob('/requests/' + encodeURIComponent(key) + '/end', {}, r.jobs)));
    row.append(name, goal, add, end);
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
    : (n.box && n.box.name ? n.box.name : (n.name || 'Session')) + ' (session ' + n.session + ')';
  r.title.textContent = label;
  const state = (n.agent && n.agent.state) || n.state || '';
  r.badge.textContent = state.replace('_', ' ');
  r.badge.className = 'badge s-' + state;
  const meta = [];
  if (n.name) meta.push('Box ' + n.name + (n.box && n.box.box_state ? ' (' + n.box.box_state + ')' : ''));
  if (n.box && n.box.branch) meta.push(n.box.branch);
  if (n.box && n.box.lifecycle) meta.push('Box ' + n.box.lifecycle + ' / ' + n.box.session_state);
  if (ended) meta.push('ended');
  r.meta.textContent = meta.join(' · ');
  r.goal.textContent = (n.box && n.box.goal) || '';
  r.goal.hidden = !r.goal.textContent;
  const url = prUrl(n.box);
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
    if (f.kind === 'artifact_published') {
      for (const r of nodes.values()) if (r.box === f.box && r.reloadArtifacts) r.reloadArtifacts();
    }
    scheduleTree();
  };
}

/* ---- static controls ---- */

$('new-start').addEventListener('click', () => {
  const out = $('new-out');
  const repo = $('new-repo').value.trim();
  const name = $('new-name').value.trim();
  const goal = $('new-goal').value.trim();
  if (!repo || !name || !goal) { show(out, 'Fill in owner/name, Box name and goal.', 'bad'); return; }
  startJob('/requests', { repo, name, goal }, out, () => { $('new-name').value = ''; $('new-goal').value = ''; });
});

$('confirm-yes').addEventListener('click', () => $('confirm').close('yes'));
$('confirm-no').addEventListener('click', () => $('confirm').close('no'));
$('confirm').addEventListener('close', () => {
  const fn = pending;
  pending = null;
  if (fn && $('confirm').returnValue === 'yes') fn();
});

refreshTree();
connect();
