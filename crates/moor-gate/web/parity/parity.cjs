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
  page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
  const data = await fixtures(page, args.mock, HIDE_MOCK, TZ);
  const missing = [];
  await page.route('**/account/api/**', (route) => {
    const u = new URL(route.request().url());
    const key = u.pathname + u.search;
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
    const a = await page.screenshot();
    fs.writeFileSync(path.join(out, s.key + '-mock.png'), a);
    errors.length = 0; missing.length = 0;
    await page.goto(portal.url + '/');
    await page.evaluate(() => { localStorage.clear(); sessionStorage.clear(); });
    await page.goto(portal.url + '/' + s.product);
    await page.addStyleTag({ content: HIDE_MOCK });
    await page.waitForSelector(s.ready || 'main:not([aria-busy])');
    if (s.productDo) await s.productDo(page);
    await settle(page);
    const alert = await page.$('[role=alert]');
    if (alert) throw new Error(s.key + ': the portal shows an alert: ' + (await alert.innerText()));
    if (missing.length) throw new Error(s.key + ': no fixture for ' + missing.join(', '));
    const b = await page.screenshot();
    fs.writeFileSync(path.join(out, s.key + '-product.png'), b);
    const r = await diff(page, a, b, path.join(out, s.key + '-diff.png'));
    rows.push({ key: s.key, ...r, errors: [...errors] });
    console.log([s.key, r.pct + '%', r.n + ' px', 'h ' + r.heights.join('/'), errors.length ? 'console errors: ' + errors.length : ''].join('\t'));
  }
  fs.writeFileSync(path.join(out, 'parity.json'), JSON.stringify(rows, null, 1));
  await page.unrouteAll({ behavior: 'ignoreErrors' });
  await browser.close();
})().catch((e) => { console.error(e.message || e); process.exit(1); });
