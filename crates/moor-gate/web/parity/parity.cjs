'use strict';
const fs = require('fs');
const path = require('path');
const os = require('os');
const { chromium } = require('playwright');
const { STATES } = require('./states.cjs');
const { fixtures } = require('./fixtures.cjs');

const argv = process.argv.slice(2);
const args = {};
for (let i = 0; i < argv.length; i += 2) args[argv[i].replace(/^--/, '')] = argv[i + 1];
if (!args.mock || !args.portal) { console.error('usage: parity.cjs --mock DIR --portal NAME [--out DIR] [--only KEY] [--size WxH]'); process.exit(2); }
const [W, H] = (args.size || '1280x900').split('x').map(Number);
const out = path.resolve(args.out || 'parity-out');
const inst = path.join(os.homedir(), '.local/state/moor-verify-live/inst', args.portal, 'portal/portal.json');
const portal = JSON.parse(fs.readFileSync(inst, 'utf8'));
const TZ = 'Asia/Hong_Kong';
// The mock's NOW, 2026-11-22 14:50 in TZ.
const NOW = Date.parse('2026-11-22T14:50:00+08:00');
const HIDE_MOCK = 'html{--mockh:0px!important} #mock{display:none!important} *,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}';

async function diff(page, a, b, file) {
  // Count pixels whose largest channel difference is over 16, in the page's own canvas: no image library needed.
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
    const blob = await c.convertToBlob();
    const buf = new Uint8Array(await blob.arrayBuffer());
    let s = ''; for (let i = 0; i < buf.length; i += 0x8000) s += String.fromCharCode.apply(null, buf.subarray(i, i + 0x8000));
    return { n, total: w * h, pct: +(100 * n / (w * h)).toFixed(2), heights: [ia.height, ib.height], png: btoa(s) };
  }, { a: a.toString('base64'), b: b.toString('base64') }).then((r) => { fs.writeFileSync(file, Buffer.from(r.png, 'base64')); delete r.png; return r; });
}

// Every text run on the page with its box and type, so a pixel difference can be named.
function texts(page) {
  return page.evaluate(() => {
    const out = [];
    const style = (el) => { const s = getComputedStyle(el); return [s.fontFamily.split(',')[0].replace(/"/g, ''), s.fontSize, s.fontWeight, s.lineHeight, s.letterSpacing, s.color].join(' '); };
    const add = (text, r, el) => { text = text.replace(/\s+/g, ' ').trim(); if (text && r.width && r.height && r.bottom > 0 && r.top < innerHeight) out.push({ text, x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height), style: style(el) }); };
    const walk = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let n; (n = walk.nextNode());) {
      const el = n.parentElement;
      if (!n.textContent.trim() || !el.checkVisibility({ visibilityProperty: true, contentVisibilityAuto: true })) continue;
      const range = document.createRange(); range.selectNodeContents(n);
      add(n.textContent, range.getBoundingClientRect(), el);
    }
    document.querySelectorAll('input[placeholder],textarea[placeholder]').forEach((el) => { if (!el.value && el.checkVisibility()) add('[placeholder] ' + el.placeholder, el.getBoundingClientRect(), el); });
    // Join runs that sit side by side on one line in one type, so "◆" + "moor" reads as "◆ moor".
    out.sort((a, b) => a.y - b.y || a.x - b.x);
    const joined = [];
    for (const t of out) {
      const p = joined[joined.length - 1];
      if (p && Math.abs(p.y - t.y) <= 1 && p.h === t.h && p.style === t.style && t.x >= p.x + p.w - 1 && t.x - (p.x + p.w) <= 6) { p.text += ' ' + t.text; p.w = t.x + t.w - p.x; } else joined.push({ ...t });
    }
    return joined;
  });
}

// Pair runs by text in reading order; print the ones that moved, changed type, or exist on one side only.
function compare(a, b) {
  const lines = [];
  const pool = new Map();
  const key = (t) => t.text.replace(/\s+/g, '');
  for (const t of b) { if (!pool.has(key(t))) pool.set(key(t), []); pool.get(key(t)).push(t); }
  for (const t of a) {
    const c = pool.get(key(t)) || [];
    const i = c.reduce((best, u, j) => (best < 0 || Math.abs(u.y - t.y) + Math.abs(u.x - t.x) < Math.abs(c[best].y - t.y) + Math.abs(c[best].x - t.x) ? j : best), -1);
    const m = i < 0 ? undefined : c.splice(i, 1)[0];
    if (!m) { lines.push(`mock only  ${t.x},${t.y} ${t.w}x${t.h}  ${JSON.stringify(t.text)}  ${t.style}`); continue; }
    m.seen = true;
    const d = [m.x - t.x, m.y - t.y, m.w - t.w, m.h - t.h];
    const moved = d.some((v) => Math.abs(v) > 0);
    if (moved || m.style !== t.style) lines.push(`moved      ${t.x},${t.y} ${t.w}x${t.h} d=${d.join(',')}  ${JSON.stringify(t.text)}${m.style !== t.style ? `  mock[${t.style}] product[${m.style}]` : ''}`);
  }
  for (const t of b) if (!t.seen) lines.push(`product only ${t.x},${t.y} ${t.w}x${t.h}  ${JSON.stringify(t.text)}  ${t.style}`);
  return lines;
}

// Hides what a state masks (each with the reason states.cjs gives) so the rest lines up, and returns the share of the page the masked elements covered.
function mask(page, list) {
  if (!list || !list.length) return Promise.resolve(0);
  return page.evaluate((sels) => {
    const els = [...new Set(sels.flatMap((s) => [...document.querySelectorAll(s)]))];
    const top = els.filter((el) => !els.some((o) => o !== el && o.contains(el)));
    let area = 0;
    for (const el of top) {
      const r = el.getBoundingClientRect();
      area += Math.max(0, Math.min(r.right, innerWidth) - Math.max(r.left, 0)) * Math.max(0, Math.min(r.bottom, innerHeight) - Math.max(r.top, 0));
    }
    for (const el of top) el.style.setProperty('display', 'none', 'important');
    return area / (innerWidth * innerHeight);
  }, list.map((m) => m.sel));
}

async function settle(page) {
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(300);
}

(async () => {
  fs.mkdirSync(out, { recursive: true });
  const browser = await chromium.connectOverCDP('http://127.0.0.1:' + portal.cdp_port);
  const ctx = browser.contexts()[0];
  // One tab: the portal skips polls while hidden, and a second tab hides it.
  const page = ctx.pages().find((p) => p.url().startsWith(portal.url)) || ctx.pages()[0];
  await page.bringToFront();
  await page.setViewportSize({ width: W, height: H });
  const cdp = await ctx.newCDPSession(page);
  await cdp.send('Page.setBypassCSP', { enabled: true });
  await cdp.send('Emulation.setTimezoneOverride', { timezoneId: TZ });
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: W, height: H, deviceScaleFactor: 1, mobile: false });
  await page.addInitScript(({ now }) => {
    if (!location.href.startsWith('http')) return;
    const Real = Date;
    class Fixed extends Real { constructor(...a) { if (a.length) super(...a); else super(now); } static now() { return now; } }
    globalThis.Date = Fixed;
  }, { now: NOW });
  const errors = [];
  page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text() + (m.location().url ? ' ' + m.location().url : '')); });
  const data = await fixtures(page, args.mock, HIDE_MOCK, TZ);
  const missing = [];
  // A predicate, not a glob: '**/account/api/**' misses the account call itself, GET /account/api.
  await page.route((u) => u.pathname === '/account/api' || u.pathname.startsWith('/account/api/'), (route) => {
    const u = new URL(route.request().url());
    const key = u.pathname + u.search;
    if (u.pathname.startsWith('/account/api/box/evidence/')) {
      const name = decodeURIComponent(u.pathname.slice('/account/api/box/evidence/'.length));
      return route.fulfill({ status: 200, contentType: 'image/png', body: Buffer.from(data.images[name] ?? data.png, 'base64') });
    }
    const body = data.api[key] ?? data.api[u.pathname];
    if (body === undefined) { missing.push(route.request().method() + ' ' + key); return route.abort(); }
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(body) });
  });
  const rows = [];
  for (const s of STATES.filter((s) => !args.only || s.key === args.only)) {
    await page.goto('file://' + path.resolve(args.mock) + '/index.html' + s.mock);
    await page.addStyleTag({ content: HIDE_MOCK });
    await page.waitForSelector('body[data-ready]');
    await page.evaluate(() => { try { sessionStorage.clear(); } catch {} });
    if (s.mockDo) await s.mockDo(page);
    await settle(page);
    const a0 = await page.screenshot();
    fs.writeFileSync(path.join(out, s.key + '-mock.png'), a0);
    const ma = await mask(page, s.mask?.mock);
    if (ma) await settle(page);
    const a = ma ? await page.screenshot() : a0;
    if (ma) fs.writeFileSync(path.join(out, s.key + '-mock-masked.png'), a);
    const ta = await texts(page);
    errors.length = 0; missing.length = 0;
    await page.goto(portal.url + '/');
    await page.evaluate(() => { localStorage.clear(); sessionStorage.clear(); });
    // A full load: a hash change that lands before the app listens for it leaves the page on the old route.
    await page.goto('about:blank');
    await page.goto(portal.url + '/' + s.product);
    await page.addStyleTag({ content: HIDE_MOCK });
    await page.waitForSelector(s.ready || 'main:not([aria-busy])').catch(async (e) => {
      fs.writeFileSync(path.join(out, s.key + '-timeout.png'), await page.screenshot());
      throw new Error(s.key + ': ' + e.message.split('\n')[0] + ' at ' + page.url() + ' (screenshot ' + s.key + '-timeout.png)');
    });
    if (s.productDo) await s.productDo(page);
    await settle(page);
    const alert = await page.$('[role=alert]');
    if (alert) throw new Error(s.key + ': the portal shows an alert: ' + (await alert.innerText()));
    if (missing.length) throw new Error(s.key + ': no fixture for ' + missing.join(', '));
    const b0 = await page.screenshot();
    fs.writeFileSync(path.join(out, s.key + '-product.png'), b0);
    const mb = await mask(page, s.mask?.product);
    if (mb) await settle(page);
    const b = mb ? await page.screenshot() : b0;
    const lines = compare(ta, await texts(page));
    fs.writeFileSync(path.join(out, s.key + '-text.txt'), lines.join('\n') + '\n');
    const r = await diff(page, a, b, path.join(out, s.key + '-diff.png'));
    const raw = ma || mb ? (await diff(page, a0, b0, path.join(out, s.key + '-diff-raw.png'))).pct : r.pct;
    const masked = +(100 * Math.max(ma, mb)).toFixed(2);
    rows.push({ key: s.key, ...r, raw, masked, masks: [...(s.mask?.mock ?? []), ...(s.mask?.product ?? [])].map((m) => m.why), errors: [...errors] });
    console.log([s.key, r.pct + '%', r.n + ' px', 'raw ' + raw + '%', 'masked ' + masked + '%', 'h ' + r.heights.join('/'), lines.length + ' text runs differ', errors.length ? 'console errors: ' + errors.length : ''].join('\t'));
  }
  fs.writeFileSync(path.join(out, 'parity.json'), JSON.stringify(rows, null, 1));
  await page.unrouteAll({ behavior: 'ignoreErrors' });
  await browser.close();
})().catch((e) => { console.error(e.message || e); process.exit(1); });
