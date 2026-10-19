(function () {
  'use strict';

  // The cookie is set; keep the token out of history and screenshots.
  if (new URLSearchParams(location.search).has('token')) {
    history.replaceState(null, '', location.pathname);
  }

  var $ = function (id) { return document.getElementById(id); };
  var DASH = '—';

  var HEADERS = { 'Content-Type': 'application/json', 'X-Moor-Csrf': '1', Accept: 'application/json' };

  function show(lines, fail) {
    var r = $('result');
    r.textContent = '';
    lines.forEach(function (l) {
      var d = document.createElement('div');
      if (fail) d.className = 'fail';
      d.textContent = l;
      r.appendChild(d);
    });
  }

  // Mirrors the CLI: `moor: <message>` then one `hint:` line each.
  function showEnvelope(env) {
    if (env && env.error) {
      var lines = [env.error.message];
      (env.error.hints || []).forEach(function (h) { lines.push('hint: ' + h); });
      show(lines, true);
    } else {
      show([env.cmd + ' ' + (env.task || '') + ': ok']);
    }
  }

  // Disables `controls` while in flight; resolves true on an ok envelope.
  function post(path, body, controls) {
    var had = document.activeElement;
    controls.forEach(function (c) { c.disabled = true; });
    return fetch(path, { method: 'POST', credentials: 'same-origin', headers: HEADERS, body: JSON.stringify(body) })
      .then(function (r) {
        if (r.status !== 200) {
          return r.text().then(function (t) { show(['HTTP ' + r.status + ': ' + t], true); return false; });
        }
        return r.json().then(function (env) {
          if (!env.error) env.task = body.task || body.name;
          showEnvelope(env);
          if (!env.error) refreshSoon();
          return !env.error;
        });
      })
      .catch(function (e) { show(['Request failed: ' + e.message], true); return false; })
      .then(function (ok) { controls.forEach(function (c) { c.disabled = false; });
        if (had && had.isConnected && controls.indexOf(had) >= 0) had.focus();
        return ok;
      });
  }

  function el(tag, attrs, text) {
    var e = document.createElement(tag);
    Object.keys(attrs || {}).forEach(function (k) { e.setAttribute(k, attrs[k]); });
    if (text) e.textContent = text;
    return e;
  }

  function labelled(form, name, what, input) {
    var id = 'in-' + what + '-' + name;
    input.id = id;
    var l = el('label', { 'for': id, 'class': 'sr-only' }, what + ' for ' + name);
    form.appendChild(l);
    form.appendChild(input);
  }

  function textForm(name, what, button, path, field) {
    var f = el('form', { 'class': 'row' });
    var input = el('input', { type: 'text', autocomplete: 'off', required: '' });
    labelled(f, name, what, input);
    var b = el('button', { type: 'submit' }, button);
    f.appendChild(b);
    f.addEventListener('submit', function (ev) {
      ev.preventDefault();
      var body = { task: name };
      body[field] = input.value;
      post(path, body, [input, b]).then(function (ok) { if (ok) input.value = ''; });
    });
    return f;
  }

  function actionsCell(name) {
    var td = el('td');
    td.appendChild(textForm(name, 'Steer', 'Send', '/steer', 'text'));
    var c = el('button', { type: 'button' }, 'Close');
    c.setAttribute('aria-label', 'Close task ' + name);
    c.addEventListener('click', function () {
      if (!window.confirm('Close task ' + name + '? This ends the Task.')) return;
      post('/close', { task: name }, [c]);
    });
    td.appendChild(c);
    return td;
  }

  // Rebuilt only when the question changes, so a typed answer survives refreshes.
  function fillActivity(box, name, agent) {
    var sig = agent && agent.state === 'needs_input'
      ? JSON.stringify([agent.question, agent.options]) : (agent ? agent.state || '' : '');
    if (box.sig === sig) return;
    box.sig = sig;
    box.textContent = '';
    if (!agent) { box.textContent = DASH; return; }
    box.appendChild(el('div', {}, (agent.state || DASH).replace(/_/g, ' ')));
    if (agent.state !== 'needs_input') return;
    if (agent.question) box.appendChild(el('div', {}, agent.question));
    var opts = el('div', { 'class': 'opts' });
    (agent.options || []).forEach(function (o, i) {
      var b = el('button', { type: 'button' }, (i + 1) + '. ' + o);
      b.addEventListener('click', function () { post('/answer', { task: name, option: i + 1 }, [b]); });
      opts.appendChild(b);
    });
    box.appendChild(opts);
    box.appendChild(textForm(name, 'Answer', 'Answer', '/answer', 'text'));
  }

  function when(iso) {
    var d = new Date(iso);
    return isNaN(d) ? (iso || DASH) : d.toLocaleString();
  }

  var rows = {};

  function buildRow(name) {
    var tr = document.createElement('tr');
    var r = { tr: tr, cells: [] };
    for (var i = 0; i < 5; i++) { r.cells.push(tr.appendChild(el('td'))); }
    r.activity = r.cells[2].appendChild(el('div'));
    tr.appendChild(actionsCell(name));
    return r;
  }

  function setCell(td, text, sub) {
    td.textContent = text;
    if (sub) td.appendChild(el('span', { 'class': 'sub' }, sub));
  }

  function render(tasks) {
    var body = document.querySelector('#tasks tbody');
    var seen = {};
    tasks.forEach(function (t, i) {
      seen[t.name] = true;
      var r = rows[t.name] || (rows[t.name] = buildRow(t.name));
      setCell(r.cells[0], t.name, t.repo);
      setCell(r.cells[1], String(t.state).replace(/_/g, ' '), t.state_reason);
      fillActivity(r.activity, t.name, t.agent);
      var b = t.box;
      setCell(r.cells[3], b ? (b.handle || b.provider) : DASH, b ? b.provider + ', ' + String(b.state).replace(/_/g, ' ') : null);
      setCell(r.cells[4], when(t.updated_at));
      if (body.children[i] !== r.tr) body.insertBefore(r.tr, body.children[i] || null);
    });
    Object.keys(rows).forEach(function (n) {
      if (!seen[n]) { rows[n].tr.remove(); delete rows[n]; }
    });
    $('tasks').hidden = tasks.length === 0;
    $('empty').hidden = tasks.length !== 0;
  }

  function loadRepos() {
    fetch('/repos', { credentials: 'same-origin', headers: { Accept: 'application/json' } })
      .then(function (r) { if (!r.ok) throw new Error('HTTP ' + r.status); return r.json(); })
      .then(function (j) {
        var sel = $('start-repo');
        var repos = j.repos || [];
        sel.textContent = '';
        repos.forEach(function (n) { sel.appendChild(el('option', { value: n }, n)); });
        $('start-submit').disabled = repos.length === 0;
        $('repos-note').textContent = repos.length ? '' : 'no repos configured';
      })
      .catch(function (e) {
        $('start-submit').disabled = true;
        $('repos-note').textContent = 'Could not load repos (' + e.message + ')';
      });
  }

  $('start').addEventListener('submit', function (ev) {
    ev.preventDefault();
    var b = $('start-submit');
    post('/start', { repo: $('start-repo').value, task: $('start-task').value, goal: $('start-goal').value }, [b, $('start-task'), $('start-goal')])
      .then(function (ok) { if (ok) { $('start-task').value = ''; $('start-goal').value = ''; } });
  });

  function showError(msg) {
    $('error').textContent = msg || '';
    $('error').hidden = !msg;
  }

  function load() {
    fetch('/tasks', { credentials: 'same-origin', headers: { Accept: 'application/json' } })
      .then(function (r) {
        if (r.status === 401) throw new Error('Not authorised. Open the URL printed by moor, including ?token=…');
        if (!r.ok) throw new Error('Could not load Tasks (HTTP ' + r.status + ')');
        return r.json();
      })
      .then(function (j) { showError(''); render(j.tasks || []); })
      .catch(function (e) { showError(e.message); });
  }

  var timer = null;
  function refreshSoon() {
    clearTimeout(timer);
    timer = setTimeout(load, 250);
  }

  function connect() {
    var es = new EventSource('/events');
    es.onopen = function () { $('status').textContent = 'Live'; };
    es.onerror = function () { $('status').textContent = 'Reconnecting…'; };
    es.onmessage = function (m) {
      try {
        var e = JSON.parse(m.data);
        $('last-event').textContent = 'Last event: ' + (e.box || '(no box)') + ' · ' + e.kind + ' · ' + new Date(e.ts).toLocaleTimeString();
      } catch (_) { /* ignore malformed frame */ }
      refreshSoon();
    };
  }

  loadRepos();
  load();
  connect();
})();
