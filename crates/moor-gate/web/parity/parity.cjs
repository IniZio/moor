'use strict';
// Task-flows mock against the portal, screen by screen: pixels, text runs, element pairs.
// usage: NODE_PATH=~/node_modules node parity.cjs --mock DIR --portal NAME [--only KEY[,KEY]] [--out DIR]
// Prints one line per screen and a final table. Exits 1 when a screen is over its ceiling (ceilings.cjs) or fails to load.
const fs = require('fs');
const path = require('path');
const os = require('os');
const { chromium } = require('playwright');
const { STATES } = require('./states.cjs');
const CEILINGS = require('./ceilings.cjs');

const argv = process.argv.slice(2);
const args = {};
for (let i = 0; i < argv.length; i += 2) args[argv[i].replace(/^--/, '')] = argv[i + 1];
if (!args.mock || !args.portal) { console.error('usage: parity.cjs --mock DIR --portal NAME [--only KEY[,KEY]] [--out DIR]'); process.exit(2); }
const W = 1440, H = 900;
const out = path.resolve(args.out || 'parity-out');
const portal = JSON.parse(fs.readFileSync(path.join(os.homedir(), '.local/state/moor-verify-live/inst', args.portal, 'portal/portal.json'), 'utf8'));
const STILL = '*,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}';
const HIDE = '#mockbar{display:none!important} ' + STILL;
const PNG1 = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAAAAAA6fptVAAAACklEQVR4nGPY0QAAAkABIGVN9fkAAAAASUVORK5CYII=', 'base64');
const mockUrl = (hash) => 'file://' + path.resolve(args.mock) + '/index.html' + hash;
const METRICS = ['px', 'differ', 'mockOnly', 'productOnly', 'pairs'];

// Pixels whose largest channel difference is over 16, counted in the page's own canvas: no image library needed.
async function diff(page, a, b, file) {
  return page.evaluate(async ({ a, b }) => {
    const load = (s) => new Promise((ok, no) => { const i = new Image(); i.onload = () => ok(i); i.onerror = no; i.src = 'data:image/png;base64,' + s; });
    const [ia, ib] = await Promise.all([load(a), load(b)]);
    const w = Math.max(ia.width, ib.width), h = Math.max(ia.height, ib.height);
    const px = (img) => { const c = new OffscreenCanvas(w, h), x = c.getContext('2d'); x.fillStyle = '#ff00ff'; x.fillRect(0, 0, w, h); x.drawImage(img, 0, 0); return x.getImageData(0, 0, w, h).data; };
    const da = px(ia), db = px(ib);
    const c = new OffscreenCanvas(w, h), x = c.getContext('2d'), od = x.createImageData(w, h);
    let n = 0;
    for (let i = 0; i < da.length; i += 4) {
      const d = Math.max(Math.abs(da[i] - db[i]), Math.abs(da[i + 1] - db[i + 1]), Math.abs(da[i + 2] - db[i + 2]));
      const g = (da[i] + da[i + 1] + da[i + 2]) / 3 * 0.3 + 178;
      if (d > 16) { n++; od.data.set([255, 0, 0, 255], i); } else od.data.set([g, g, g, 255], i);
    }
    x.putImageData(od, 0, 0);
    const buf = new Uint8Array(await (await c.convertToBlob()).arrayBuffer());
    let s = ''; for (let i = 0; i < buf.length; i += 0x8000) s += String.fromCharCode.apply(null, buf.subarray(i, i + 0x8000));
    return { pct: +(100 * n / (w * h)).toFixed(2), png: btoa(s) };
  }, { a: a.toString('base64'), b: b.toString('base64') }).then((r) => { fs.writeFileSync(file, Buffer.from(r.png, 'base64')); return r.pct; });
}

// Every text run with its type and the nearest box around it (border, background or shadow), so a pixel difference can be named.
function texts(page) {
  return page.evaluate(() => {
    const out = [];
    const type = (el) => { const s = getComputedStyle(el); return [s.fontFamily.split(',')[0].replace(/"/g, ''), s.fontSize, s.fontWeight, 'lh ' + s.lineHeight, s.color].join(' '); };
    const boxOf = (el) => {
      for (let e = el; e && !['MAIN', 'BODY', 'HTML'].includes(e.tagName) && e.id !== 'main'; e = e.parentElement) {
        const s = getComputedStyle(e);
        const bw = ['Top', 'Right', 'Bottom', 'Left'].map((k) => s['border' + k + 'Width']);
        const has = bw.some((w) => parseFloat(w) > 0) || (s.backgroundColor !== 'rgba(0, 0, 0, 0)' && s.backgroundColor !== 'transparent') || s.boxShadow !== 'none';
        if (!has) continue;
        const r = e.getBoundingClientRect();
        return [Math.round(r.width) + 'x' + Math.round(r.height), 'pad ' + s.padding, 'bw ' + bw.join(' '), 'bc ' + s.borderColor, 'rad ' + s.borderRadius, 'bg ' + s.backgroundColor, s.boxShadow === 'none' ? '' : 'shadow'].join(' | ');
      }
      return '-';
    };
    const add = (text, r, el) => { text = text.replace(/\s+/g, ' ').trim(); if (text && r.width && r.height && r.bottom > 0 && r.top < innerHeight) out.push({ text, x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height), style: type(el), box: boxOf(el) }); };
    const walk = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let n; (n = walk.nextNode());) {
      const el = n.parentElement;
      if (!n.textContent.trim() || el.closest('#mockbar, script, style') || !el.checkVisibility({ visibilityProperty: true })) continue;
      const range = document.createRange(); range.selectNodeContents(n);
      add(n.textContent, range.getBoundingClientRect(), el);
    }
    document.querySelectorAll('textarea').forEach((el) => { if (el.value && el.checkVisibility()) add('[textarea] ' + el.value, el.getBoundingClientRect(), el); });
    document.querySelectorAll('input[placeholder],textarea[placeholder]').forEach((el) => { if (!el.value && el.checkVisibility()) add('[placeholder] ' + el.placeholder, el.getBoundingClientRect(), el); });
    out.sort((a, b) => a.y - b.y || a.x - b.x);
    // Join runs side by side on one line in one type, so "◆" + "moor" reads as "◆ moor".
    const joined = [];
    for (const t of out) { const p = joined[joined.length - 1]; if (p && Math.abs(p.y - t.y) <= 1 && p.h === t.h && p.style === t.style && t.x >= p.x + p.w - 1 && t.x - (p.x + p.w) <= 6) { p.text += ' ' + t.text; p.w = t.x + t.w - p.x; } else joined.push({ ...t }); }
    return joined;
  });
}

// Pair runs by text, nearest first. OK = same place, type and box.
function compare(a, b) {
  const lines = [], pool = new Map(), key = (t) => t.text.replace(/^\[textarea\] /, '').replace(/\s+/g, '').toLowerCase();
  const n = { ok: 0, differ: 0, mockOnly: 0, productOnly: 0 };
  for (const t of b) { if (!pool.has(key(t))) pool.set(key(t), []); pool.get(key(t)).push(t); }
  for (const t of a) {
    const c = pool.get(key(t)) || [];
    const i = c.reduce((best, u, j) => (best < 0 || Math.abs(u.y - t.y) + Math.abs(u.x - t.x) < Math.abs(c[best].y - t.y) + Math.abs(c[best].x - t.x) ? j : best), -1);
    const m = i < 0 ? undefined : c.splice(i, 1)[0];
    if (!m) { n.mockOnly++; lines.push('MOCK ONLY  ' + t.x + ',' + t.y + ' ' + t.w + 'x' + t.h + '  ' + JSON.stringify(t.text.slice(0, 90)) + '  [' + t.style + ']  box ' + t.box); continue; }
    m.seen = true;
    const d = [m.x - t.x, m.y - t.y, m.w - t.w, m.h - t.h];
    const same = d.every((v) => v === 0) && m.style === t.style && m.box === t.box;
    n[same ? 'ok' : 'differ']++;
    lines.push((same ? 'OK' : 'DIFF') + '  ' + JSON.stringify(t.text.slice(0, 70)) + ' @' + t.x + ',' + t.y + ' d=' + d.join(','));
    if (m.style !== t.style) lines.push('     type  mock[' + t.style + ']  product[' + m.style + ']');
    if (m.box !== t.box) lines.push('     box   mock[' + t.box + ']\n           product[' + m.box + ']');
  }
  for (const t of b) if (!t.seen) { n.productOnly++; lines.push('PRODUCT ONLY ' + t.x + ',' + t.y + ' ' + t.w + 'x' + t.h + '  ' + JSON.stringify(t.text.slice(0, 90)) + '  [' + t.style + ']  box ' + t.box); }
  return { n, lines };
}

// The computed box and type of the first match of each pair's selector on one side; side 1 is the mock, 2 the product.
const PROPS = ['display', 'font-family', 'font-size', 'font-weight', 'font-style', 'line-height', 'color', 'background-color', 'padding', 'border-top', 'border-bottom', 'border-left', 'border-radius', 'box-shadow', 'gap', 'text-decoration-line', 'text-decoration-style', 'opacity', 'grid-template-columns', 'max-width', 'height'];
function styles(page, pairs, side) {
  return page.evaluate(({ pairs, props, side }) => {
    const res = {};
    for (const p of pairs) {
      const sel = p[side];
      if (!sel) { res[p[0]] = null; continue; }
      const pseudo = sel.endsWith('::before') ? '::before' : null;
      const el = document.querySelector(pseudo ? sel.slice(0, -8) : sel);
      if (!el || !el.checkVisibility()) { res[p[0]] = 'absent'; continue; }
      const cs = getComputedStyle(el, pseudo);
      const r = el.getBoundingClientRect();
      const o = { box: [r.x, r.y, r.width, r.height].map(Math.round).join(','), text: (pseudo ? cs.content : el.innerText || el.value || '').replace(/\s+/g, ' ').trim().slice(0, 70) };
      for (const k of props) { let v = cs.getPropertyValue(k); if (k === 'font-family') v = v.split(',')[0].replace(/"/g, ''); o[k] = v; }
      res[p[0]] = o;
    }
    return res;
  }, { pairs, props: PROPS, side });
}
function comparePairs(pairs, a, b) {
  const lines = []; let n = 0;
  for (const [label] of pairs) {
    const x = a[label], y = b[label];
    if (x == null && y == null) continue;
    if (x == null || x === 'absent') {
      if (y && y !== 'absent') { n++; lines.push('PRODUCT ONLY  ' + label + '  box ' + y.box + '  "' + y.text + '"'); } else lines.push('both absent  ' + label);
      continue;
    }
    if (y == null || y === 'absent') { n++; lines.push('MOCK ONLY  ' + label + '  box ' + x.box + '  "' + x.text + '"'); continue; }
    const d = ['box', ...PROPS].flatMap((k) => (x[k] === y[k] ? [] : [k + ': ' + x[k] + ' -> ' + y[k]]));
    if (d.length) n++;
    lines.push((d.length ? 'DIFF  ' : 'ok    ') + label + (d.length ? '\n      ' + d.join('\n      ') : ''));
  }
  return { n, lines };
}

const settle = async (page) => {
  await page.evaluate(() => Promise.all([document.fonts.ready, ...[...document.images].map((i) => i.decode().catch(() => {}))]));
  await page.waitForTimeout(400);
};

async function pictures(page) {
  const { PICS } = require('./fixtures/task.cjs');
  const images = {};
  for (const [name, hash, sel] of PICS) {
    await page.goto(mockUrl(hash)); await page.addStyleTag({ content: HIDE }); await page.waitForSelector('html[data-ready]');
    const el = page.locator(sel).first();
    if (await el.count()) images[name] = await el.screenshot();
  }
  return images;
}

async function measure(page, s, data, missing, images) {
  const pairs = s.pairs || [];
  await page.goto('about:blank');
  await page.goto(mockUrl(s.mock));
  await page.addStyleTag({ content: HIDE });
  await page.waitForSelector('html[data-ready]');
  if (s.mockDo) await s.mockDo(page);
  await settle(page);
  const a = await page.screenshot();
  fs.writeFileSync(path.join(out, s.key + '-mock.png'), a);
  const ta = await texts(page), sa = await styles(page, pairs, 1);
  await page.evaluate(() => { try { sessionStorage.clear(); } catch {} });

  missing.length = 0;
  await page.goto(portal.url + '/');
  await page.evaluate(({ store, now }) => {
    localStorage.clear(); sessionStorage.clear();
    localStorage.setItem('moor.theme', 'light'); localStorage.setItem('audit.now', String(now));
    for (const [k, v] of Object.entries(store.session ?? {})) sessionStorage.setItem(k, v);
    for (const [k, v] of Object.entries(store.local ?? {})) localStorage.setItem(k, v);
  }, { store: s.store ?? {}, now: Date.parse(s.now) });
  // A full load: a hash change that lands before the app listens for it leaves the page on the old route.
  await page.goto('about:blank');
  await page.goto(portal.url + '/' + s.product);
  await page.addStyleTag({ content: STILL });
  await page.waitForSelector(s.ready || 'main:not([aria-busy])', { timeout: 10000 }).catch(async (e) => {
    fs.writeFileSync(path.join(out, s.key + '-timeout.png'), await page.screenshot());
    throw new Error('product never showed ' + (s.ready || 'main') + ' (' + e.message.split('\n')[0] + ')');
  });
  if (s.productDo) await s.productDo(page);
  if (s.toEnd) await page.evaluate(() => document.querySelectorAll('main *').forEach((e) => { if (e.scrollHeight > e.clientHeight + 8 && /auto|scroll/.test(getComputedStyle(e).overflowY)) e.scrollTop = e.scrollHeight; }));
  await settle(page);
  const alert = await page.$('[role=alert]');
  if (alert) throw new Error('the portal shows an alert: ' + (await alert.innerText()));
  if (missing.length) throw new Error('no fixture for ' + [...new Set(missing)].join(', '));
  const b = await page.screenshot();
  fs.writeFileSync(path.join(out, s.key + '-product.png'), b);
  const text = compare(ta, await texts(page));
  const pair = comparePairs(pairs, sa, await styles(page, pairs, 2));
  fs.writeFileSync(path.join(out, s.key + '-text.txt'), (s.note ? '# ' + s.note + '\n' : '') + text.lines.join('\n') + '\n');
  fs.writeFileSync(path.join(out, s.key + '-pairs.txt'), pair.lines.join('\n') + '\n');
  const px = await diff(page, a, b, path.join(out, s.key + '-diff.png'));
  return { key: s.key, px, differ: text.n.differ, ok: text.n.ok, mockOnly: text.n.mockOnly, productOnly: text.n.productOnly, pairs: pair.n };
}

const fmt = (r) => r.key.padEnd(9) + (r.px.toFixed(2) + '%').padStart(7) + '  text ' + r.ok + ' ok / ' + r.differ + ' differ / ' + r.mockOnly + ' mock only / ' + r.productOnly + ' product only  pairs ' + r.pairs;

(async () => {
  fs.mkdirSync(out, { recursive: true });
  const states = STATES.filter((s) => !args.only || args.only.split(',').includes(s.key));
  if (!states.length) { console.error('no screen matches --only ' + args.only); process.exit(2); }
  const browser = await chromium.connectOverCDP('http://127.0.0.1:' + portal.cdp_port);
  const ctx = browser.contexts()[0];
  // One tab: the portal skips polls while hidden, and a second tab hides it.
  const page = ctx.pages().find((p) => p.url().startsWith(portal.url)) || ctx.pages()[0];
  await page.bringToFront();
  await page.setViewportSize({ width: W, height: H });
  const cdp = await ctx.newCDPSession(page);
  await cdp.send('Page.setBypassCSP', { enabled: true });
  await cdp.send('Emulation.setTimezoneOverride', { timezoneId: 'Asia/Hong_Kong' });
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: W, height: H, deviceScaleFactor: 1, mobile: false });
  await cdp.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'light' }] });
  // The product's clock stands at the state's time; the mock keeps its own.
  await page.addInitScript(() => {
    const now = Number(localStorage.getItem('audit.now') || 0);
    if (!now || !location.href.startsWith('http')) return;
    const Real = Date;
    class Fixed extends Real { constructor(...a) { if (a.length) super(...a); else super(now); } static now() { return now; } }
    globalThis.Date = Fixed;
  });
  const images = states.some((s) => s.area === 'task') ? await pictures(page) : {};
  let data = {};
  const missing = [];
  // A predicate, not a glob: '**/account/api/**' misses the account call itself, GET /account/api.
  await page.route((u) => u.pathname === '/account/api' || u.pathname.startsWith('/account/api/'), (route) => {
    const u = new URL(route.request().url()), k = u.pathname + u.search;
    const ev = '/account/api/box/evidence/';
    if (u.pathname.startsWith(ev)) return route.fulfill({ status: 200, contentType: 'image/png', body: images[decodeURIComponent(u.pathname.slice(ev.length))] ?? PNG1 });
    const body = data[k] ?? data[u.pathname];
    if (body === undefined) { missing.push(route.request().method() + ' ' + k); return route.fulfill({ status: 404, contentType: 'application/json', body: '{"error":"not in the fixture"}' }); }
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(body) });
  });
  const rows = [], failed = [];
  for (const s of states) {
    try {
      data = require('./fixtures/' + s.area + '.cjs').api(s, args.mock);
      const r = await measure(page, s, data, missing, images);
      rows.push(r);
      console.log(fmt(r));
    } catch (e) {
      failed.push({ key: s.key, why: e.message.split('\n')[0] });
      console.log(s.key.padEnd(9) + ' ERROR ' + e.message.split('\n')[0]);
    }
  }
  fs.writeFileSync(path.join(out, 'parity.json'), JSON.stringify(rows, null, 1));
  await page.unrouteAll({ behavior: 'ignoreErrors' });
  await browser.close();

  console.log('\nscreen    ' + 'pixels'.padStart(8) + ' (max)  ' + 'differ'.padStart(10) + '  ' + 'mock only'.padStart(12) + '  ' + 'product only'.padStart(15) + '  ' + 'pairs'.padStart(9) + '  verdict');
  let over = 0;
  for (const r of rows) {
    const c = CEILINGS[r.key];
    const cell = (k, w) => (c ? r[k] + '/' + c[k] : String(r[k])).padStart(w);
    const bad = c ? METRICS.filter((k) => r[k] > c[k]) : METRICS;
    const slack = c && !bad.length && METRICS.some((k) => r[k] < c[k]);
    if (bad.length) over++;
    console.log(r.key.padEnd(9) + (r.px.toFixed(2) + '%').padStart(8) + (c ? '/' + c.px.toFixed(2) : '').padEnd(8) + cell('differ', 10) + '  ' + cell('mockOnly', 12) + '  ' + cell('productOnly', 15) + '  ' + cell('pairs', 9) + '  ' + (!c ? 'NO CEILING' : bad.length ? 'OVER ' + bad.join(',') : slack ? 'ok, ceiling can drop' : 'ok'));
  }
  console.log('\n' + rows.length + ' screens, ' + over + ' over ceiling, ' + failed.length + ' failed');
  process.exit(over || failed.length ? 1 : 0);
})().catch((e) => { console.error(e.stack || e); process.exit(1); });
