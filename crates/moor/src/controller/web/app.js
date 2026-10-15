(function () {
  'use strict';

  // The cookie is set; keep the token out of history and screenshots.
  if (new URLSearchParams(location.search).has('token')) {
    history.replaceState(null, '', location.pathname);
  }

  var $ = function (id) { return document.getElementById(id); };
  var DASH = '—';

  function cell(tr, text, sub) {
    var td = document.createElement('td');
    td.textContent = text;
    if (sub) {
      var s = document.createElement('span');
      s.className = 'sub';
      s.textContent = sub;
      td.appendChild(s);
    }
    tr.appendChild(td);
  }

  function activity(agent) {
    if (!agent) return DASH;
    var text = agent.state || DASH;
    if (agent.question) text += ': ' + agent.question;
    if (agent.options && agent.options.length) text += ' [' + agent.options.join(' / ') + ']';
    return text;
  }

  function when(iso) {
    var d = new Date(iso);
    return isNaN(d) ? (iso || DASH) : d.toLocaleString();
  }

  function render(tasks) {
    var body = document.querySelector('#tasks tbody');
    body.textContent = '';
    tasks.forEach(function (t) {
      var tr = document.createElement('tr');
      cell(tr, t.name, t.repo);
      cell(tr, String(t.state).replace(/_/g, ' '), t.state_reason);
      cell(tr, activity(t.agent));
      var b = t.box;
      cell(tr, b ? (b.handle || b.provider) : DASH, b ? b.provider + ', ' + String(b.state).replace(/_/g, ' ') : null);
      cell(tr, when(t.updated_at));
      body.appendChild(tr);
    });
    $('tasks').hidden = tasks.length === 0;
    $('empty').hidden = tasks.length !== 0;
  }

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
        $('last-event').textContent = 'Last event: ' + e.task + ' · ' + e.kind + ' · ' + new Date(e.ts).toLocaleTimeString();
      } catch (_) { /* ignore malformed frame */ }
      refreshSoon();
    };
  }

  load();
  connect();
})();
