// The Agent part's terminal: xterm.js on the task's terminal websocket, the steering seat and the
// "Type to the agent" line. Binary messages are terminal bytes, text messages are seat and end notices.
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';

const el = document.getElementById('term');
if (el) start(el);

function parseNotice(text) {
  const m = JSON.parse(text);
  if (m.ended) return { ended: String(m.ended) };
  return { seat: m.seat === 'steer' ? 'steer' : 'watch', holder: String(m.holder || ''), watchers: Array.isArray(m.watchers) ? m.watchers.map(String) : [] };
}

function start(el) {
  const task = el.dataset.task;
  const me = el.dataset.me;
  const readonly = el.dataset.readonly === 'true';
  el.textContent = '';
  const term = new Terminal({
    scrollback: 10000, fontFamily: 'JBMono, monospace', fontSize: 12, cursorBlink: true,
    theme: { background: '#171411', foreground: '#efeae2' },
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(el);
  fit.fit();
  const enc = new TextEncoder();
  let ws = null;
  let want = 'watch';
  let seat = '';
  let first = true;
  let wait = 1000;
  let keysText = 'Not connected.';
  let connected = false;
  let pendingKeys = [];
  let pendingLine = null;
  let lastHuman = -Infinity;
  const human = () => { lastHuman = performance.now(); };
  el.addEventListener('keydown', human, true);
  el.addEventListener('paste', human, true);
  el.addEventListener('beforeinput', human, true);
  el.addEventListener('mousedown', human, true);
  el.addEventListener('wheel', human, true);

  const show = (text) => {
    keysText = text;
    const k = document.getElementById('keys');
    if (k) k.textContent = text;
  };
  const inputOn = () => {
    const i = document.getElementById('tin');
    if (i) i.disabled = readonly || !connected;
  };
  const steering = () => ws && ws.readyState === WebSocket.OPEN && seat === 'steer';

  function connect(mode) {
    want = mode;
    const q = new URLSearchParams({ mode, cols: term.cols, rows: term.rows });
    const s = new WebSocket((location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/api/tasks/' + task + '/terminal?' + q);
    s.binaryType = 'arraybuffer';
    ws = s;
    s.onmessage = (e) => {
      if (e.data instanceof ArrayBuffer) {
        term.write(new Uint8Array(e.data));
        return;
      }
      notice(parseNotice(e.data));
    };
    s.onclose = (e) => {
      if (ws !== s) return;
      ws = null;
      connected = false;
      inputOn();
      if (e.code === 4002) {
        seat = '';
        connect('watch');
        return;
      }
      setTimeout(() => connect(seat === 'steer' ? 'steer' : 'watch'), e.code === 4001 ? 0 : wait);
      wait = Math.min(wait * 2, 10000);
    };
  }

  function notice(m) {
    if (m.ended) {
      connected = false;
      inputOn();
      show('Not connected: ' + m.ended + '.');
      return;
    }
    connected = true;
    wait = 1000;
    seat = m.seat;
    inputOn();
    show(seatLine(m));
    if (first && m.seat === 'watch' && !m.holder && !readonly) {
      first = false;
      take();
      return;
    }
    first = false;
    if (seat === 'steer') {
      ws.send(JSON.stringify({ cols: term.cols, rows: term.rows }));
      pendingKeys.forEach((b) => ws.send(b));
      pendingKeys = [];
      if (pendingLine !== null) {
        const line = pendingLine;
        pendingLine = null;
        post(line);
      }
    }
  }

  function seatLine(m) {
    const others = m.watchers.filter((w) => w !== me);
    const watching = others.length === 0 ? '' : ' ' + others.join(', ') + (others.length === 1 ? ' is' : ' are') + ' watching.';
    if (m.seat === 'steer') return 'You have the keyboard.' + watching;
    if (!m.holder) return 'No one is typing.' + watching;
    if (m.holder === me) return 'You have the keyboard in another tab.' + watching;
    return m.holder + ' has the keyboard. Type to take it.' + watching;
  }

  // take reattaches in steer mode, which takes the seat from whoever holds it.
  function take() {
    const old = ws;
    ws = null;
    if (old) old.close();
    seat = '';
    connect('steer');
  }

  // keys sends what a person typed. What xterm.js answers by itself to the session's queries is dropped: the
  // desk's terminal answers those, and a reply would reach the session as typing and reopen a done task.
  function keys(b) {
    if (readonly || performance.now() - lastHuman > 1000) return;
    if (steering()) {
      ws.send(b);
      return;
    }
    pendingKeys.push(b);
    if (want !== 'steer') take();
  }

  async function post(text) {
    const r = await fetch('/api/tasks/' + task + '/input', {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ text }),
    });
    if (r.ok) {
      const i = document.getElementById('tin');
      if (i) i.value = '';
      return;
    }
    const body = await r.json().catch(() => ({}));
    show(body.error || 'The line was not sent.');
  }

  term.onData((d) => keys(enc.encode(d)));
  term.onBinary((d) => keys(Uint8Array.from(d, (c) => c.charCodeAt(0))));
  term.onResize(({ cols, rows }) => {
    if (steering()) ws.send(JSON.stringify({ cols, rows }));
  });
  new ResizeObserver(() => fit.fit()).observe(el);

  document.addEventListener('submit', (e) => {
    const form = e.target;
    if (!(form instanceof HTMLFormElement) || !form.classList.contains('tin')) return;
    e.preventDefault();
    const text = form.querySelector('input').value;
    if (!text.trim() || readonly) return;
    if (steering()) {
      post(text);
      return;
    }
    pendingLine = text;
    take();
  });
  document.addEventListener('htmx:afterSwap', () => {
    show(keysText);
    inputOn();
  });

  connect('watch');
}
