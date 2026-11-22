'use strict';
// Lists what the mock marks as built, partly built or not built, and every control it offers, per scene,
// so each can be checked against the product. Usage: inventory.cjs --mock DIR --portal NAME [--match REGEX]
const fs = require('fs');
const path = require('path');
const os = require('os');
const { chromium } = require('playwright');

const argv = process.argv.slice(2);
const args = {};
for (let i = 0; i < argv.length; i += 2) args[argv[i].replace(/^--/, '')] = argv[i + 1];
if (!args.mock || !args.portal) { console.error('usage: inventory.cjs --mock DIR --portal NAME [--match REGEX]'); process.exit(2); }
const portal = JSON.parse(fs.readFileSync(path.join(os.homedir(), '.local/state/moor-verify-live/inst', args.portal, 'portal/portal.json'), 'utf8'));
const match = new RegExp(args.match || '^(Tasks|Inbox|Writing|Reading|Task)');

(async () => {
  const browser = await chromium.connectOverCDP('http://127.0.0.1:' + portal.cdp_port);
  const page = await browser.contexts()[0].newPage();
  const base = 'file://' + path.resolve(args.mock) + '/index.html';
  await page.goto(base + '#/tasks');
  await page.waitForSelector('body[data-ready]');
  const routes = (await page.evaluate(() => window.MOCK_ROUTES)).filter(([name]) => match.test(name));
  const seen = new Map();
  for (const [name, hash] of routes) {
    await page.goto(base + hash + (hash.includes('?') ? '&' : '?') + 'built=1');
    await page.waitForSelector('body[data-ready]');
    const found = await page.evaluate(() => {
      const say = (el) => (el.innerText || el.getAttribute('aria-label') || el.getAttribute('placeholder') || el.title || '').replace(/\s+/g, ' ').trim().slice(0, 90);
      const out = [];
      for (const b of document.querySelectorAll('main .bt, #peek .bt, #inbox .bt, header .bt, .bt')) {
        if (b.closest('#mock')) continue;
        const host = b.parentElement;
        out.push(b.textContent + ' | ' + say(host).replace(b.textContent, '').trim());
      }
      for (const c of document.querySelectorAll('button, a[href], input, textarea, select, summary, [contenteditable], [data-act]')) {
        if (c.closest('#mock') || !c.checkVisibility()) continue;
        const d = Object.entries(c.dataset).map(([k, v]) => k + '=' + v).join(' ');
        out.push('control | ' + c.tagName.toLowerCase() + ' ' + JSON.stringify(say(c)) + (d ? ' [' + d + ']' : ''));
      }
      return out;
    });
    for (const f of found) { if (!seen.has(f)) seen.set(f, []); seen.get(f).push(name); }
  }
  const rows = [...seen].sort(([a], [b]) => a.localeCompare(b)).map(([f, where]) => f + '  (' + where.slice(0, 3).join('; ') + (where.length > 3 ? '; +' + (where.length - 3) : '') + ')');
  console.log(rows.join('\n'));
  await page.close();
  await browser.close();
})().catch((e) => { console.error(e.message || e); process.exit(1); });
