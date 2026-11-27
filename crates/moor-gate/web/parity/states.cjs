'use strict';
// Every task-flows screen: the mock's hash, the product's route, and what to do on each side before measuring.
// area picks fixtures/AREA.cjs; now is the clock the product stands at; pairs are [label, mock selector, product selector], null where one side has none.
// No screen hides anything. A mask on either side needs a reason written beside it.
// Not covered, no product counterpart: a2 (no Read it step; the open write field is its nearest state), a5 (no artifact page; Open shows the raw file).
const click = (sel) => (page) => page.click(sel);
const D22 = '2026-11-22T14:50:00+08:00', D24 = '2026-11-24T11:12:00+08:00', D24C = '2026-11-24T14:10:00+08:00', D24A = '2026-11-24T15:00:00+08:00', D25 = '2026-11-25T09:20:00+08:00';

const { C1_PROMPT, CHAT_SENTENCE } = require('./fixtures/automation.cjs');
const BRIEF = 'A date picker for the booking screen, in Storybook. Three states: default, selected, unavailable. Then send it for review.';
const H1DRAFT = 'Make it 3% above 500 and show me the booking screen with a 600 booking.';
const home = { area: 'task', product: '#/new', ready: '.t-wform', now: D24, inbox: true, store: { session: { 'moor.draft.new-task': BRIEF }, local: { 'moor.lastRepos': '["acme/clinic-prototype"]' } } };
const box = (id, extra) => ({ key: id, area: 'task', mock: '#' + id, product: '#/box/' + id, ready: '.t-th', now: D24, ...extra });

const toggle = (page) => page.click('.ftog');
const rest = (id) => (page) => page.hover('.pmarks [data-id="' + id + '"]');
const typeEnd = async (page) => { await page.fill('textarea.promptbox', C1_PROMPT + ' ' + CHAT_SENTENCE); await page.evaluate(() => document.activeElement.blur()); };
const auto = (key, mock, product, extra) => ({ key, area: 'automation', mock, product, ready: '.auto .pbox', now: D24A, pairs: AUTO_PAIRS, ...extra });

const RULES_WORDS = 'Let Boxes in this repo comment on pull requests.';
const typeRules = (page) => page.fill('.rules-ask textarea', RULES_WORDS);
const rules = (key, extra) => ({ key, area: 'rules', mock: '#' + key, product: '#/repos/acme/web/rules', ready: '.rules-file', now: D22, pairs: RULES_PAIRS, ...extra });

const AUTO_PAIRS = [
  ['top bar', '#top .bar', 'header'],
  ['top bar current item', '#top a[aria-current]', 'header a[aria-current=page]'],
  ['page column', '#main .wrap', 'main'],
  ['back link', null, '.auto .back'],
  ['title h1', '#main .pagehead h1', '.auto .pagehead h1'],
  ['header actions', null, '.auto .wf-actions'],
  ['Run now button', null, '.auto .wf-actions button'],
  ['trigger line', '.pbox .trig', '.auto .trig'],
  ['trigger value', '.pbox .trig .psp', '.auto .trig .t-mono'],
  ['trigger PR pill', null, '.auto .trig .pill'],
  ['revision line', '.pbox .rev', '.auto .rev'],
  ['revision Edit link', '.pbox .rev .lnk', '.auto .rev a'],
  ['grid', '.pgrid', '.auto .pgrid'],
  ['words box', '.pleft .promptbox', '.auto .pmarks, .auto .promptbox'],
  ['words solid mark', '.pleft .psp.k-enf', '.auto .pleft .psp.k-enf'],
  ['words dashed mark', '.pleft .psp.k-ask', '.auto .pleft .psp.k-ask'],
  ['words dotted mark', '.pleft .psp.k-dot', null],
  ['legend', '.pleft .legend', '.auto .legend'],
  ['question box', '.pleft .ask', null],
  ['question text', '.pleft .ask .aq', null],
  ['question button', '.pleft .ask .btn', null],
  ['Repos heading', '.pleft h2.sub', '.auto .pleft h2.sub'],
  ['Repos rows', '.pleft .rows', '.auto .pleft .rows'],
  ['Repos row', '.pleft a.rrow', '.auto a.rrow'],
  ['Repos row dot', '.pleft a.rrow .rdot', '.auto a.rrow .t-rdot'],
  ['Repos row name', '.pleft a.rrow .rfirst', '.auto a.rrow .t-mono'],
  ['Repos withheld count', '.pleft a.rrow .rtime', null],
  ['file panel', '.file', '.auto .file'],
  ['file name', '.file .fname', '.auto .fname'],
  ['file body', '.file .fbody', '.auto .fbody'],
  ['file line 1', '.file .fl:nth-child(1)', '.auto .fl:nth-child(1)'],
  ['file line number', '.file .fl:nth-child(1)::before', '.auto .fl:nth-child(1)::before'],
  ['file comment line', '.file .fl.cm', '.auto .fl.comment'],
  ['file trailing comment', '.file .fc', '.auto .fc'],
  ['file separator', '.file .fl.fsep', '.auto .fl.sep'],
  ['file lit line', '.file .fl.flit, .file .fl.fadd', '.auto .fl.fadd'],
  ['file new tag', '.file .pnew', '.auto .fl .pnew'],
  ['foot row', '.pfoot', '.auto .pfoot'],
  ['Hide file toggle', '.pfoot .ftog', '.auto .ftog'],
  ['foot primary button', '.pfoot .btn.primary', '.auto .pfoot .btn.primary'],
  ['foot other button', '.pfoot .btn:not(.primary)', '.auto .pfoot .btn:not(.primary)'],
  ['Plan section', null, '.auto .plan-view'],
  ['Runs heading', '#main .wrap > h2.sub', '.auto > h2.sub'],
  ['run group head', '#main .dayhead', '.auto .dayhead'],
  ['run group head link', null, '.auto .dayhead a'],
  ['run rows list', '#main .wrap > .rows', '.auto ul.runs'],
  ['run row', '#main a.run, #main div.run', '.auto ul.runs li > a'],
  ['run status mark', '#main .run .smk', '.auto ul.runs .smk'],
  ['run repo or sha', '#main .run .rfirst', '.auto ul.runs .t-mono'],
  ['run sha mono', '#main .run .rfirst', '.auto ul.runs .t-mono'],
  ['run outcome words', '#main .run .rnow', '.auto ul.runs .outcome'],
  ['run time', '#main .run .rtime', '.auto ul.runs .rtime'],
  ['run failed words', '#main .run .rnow.n-failed', '.auto ul.runs .outcome.n-failed'],
];
const RULES_PAIRS = [
  ['top bar', '#top .bar', 'header.top .bar'],
  ['brand', '#top .brand', 'header.top .brand'],
  ['nav current (Team)', '#top a[aria-current]', 'header.top a[aria-current]'],
  ['nav Tasks', '#top .bar a.nav', 'header.top .bar a.nav'],
  ['find', '#top .find', 'header.top .find'],
  ['find kbd', '#top .findkbd', 'header.top .findkbd'],
  ['inbox', '#top .nav.inbox', 'header.top .nav.inbox'],
  ['avatar', '#top .avatar', 'header.top .avatar'],
  ['main', '#main', 'main'],
  ['page', '.rpage', '.rules-page'],
  ['h1', '.rpage h1', '.rules-page h1'],
  ['revision line', '.rpage > .rev', '.rules-rev'],
  ['grid', '.rgrid', '.rules-grid'],
  ['file panel', '.rfile', '.rules-file'],
  ['file name', '.rfile .fname', '.rules-head'],
  ['file body', '.rfile .fbody', '.rules-body, .rules-diff'],
  ['file line', '.rfile .fl', '.rules-body textarea, .rules-diff .dl'],
  ['line number', '.rfile .fl::before', '.rules-gutter, .rules-diff .dn'],
  ['comment line', '.rfile .fl.cm', '.rules-diff .dl:nth-child(46)'],
  ['trailing comment', '.rfile .fc', null],
  ['new line', '.rfile .fl.fadd', '.rules-diff .dl.add'],
  ['new tag', '.rfile .pnew', '.rules-diff .dtag'],
  ['file foot', '.rfile .ffoot', '.rules-foot'],
  ['Save', '.ffoot .btn.primary', '.rules-foot button.primary'],
  ['Discard', '.ffoot a.btn', '.rules-foot button:last-child'],
  ['right column', '.rcol', '.rules-seen'],
  ['heading Withheld', '.rcol h2.sub', '.rules-seen h3'],
  ['heading Dialled', '.rcol h2.sub:nth-of-type(2)', '.rules-seen h3:nth-of-type(2)'],
  ['withheld list', '.rcol .rows', '.rules-seen .seen-rows'],
  ['withheld row', '.rcol .wr', '.rules-seen .seen-rows .seen-row'],
  ['withheld row (lit)', '.rcol .wr.on', null],
  ['withheld request', '.rcol .wr .mono', '.rules-seen .seen-rows li code'],
  ['withheld host', '.rcol .wr .mx', '.rules-seen .seen-rows li code.host'],
  ['withheld count', '.rcol .wr .mn', '.rules-seen .seen-rows li .seen-n'],
  ['seen note', null, '.rules-seen .seen-note'],
  ['dialled row', '.rcol .dr', '.rules-seen ul:nth-of-type(2) li'],
  ['dialled host', '.rcol .dr .mono', '.rules-seen ul:nth-of-type(2) li code'],
  ['dialled count', '.rcol .dr .mn', '.rules-seen ul:nth-of-type(2) li .seen-n'],
  ['agent line', '.rpage > .aline', '.rules-agent'],
  ['agent mark', '.rpage > .aline .amark', '.rules-mark'],
  ['agent line link', null, '.rules-agent a'],
  ['composer', '.rpage .composer', '.rules-ask'],
  ['composer inner', '.rpage .cin', null],
  ['composer field', '.rpage .composer textarea', '.rules-ask textarea'],
  ['Send', '.rpage .composer .btn', '.rules-ask button'],
];

const STATES = [
  { key: 'a1', mock: '#a1', ...home },
  box('a3', { inbox: true }),
  box('a4'),
  auto('b1', '#b1', '#/workflows/deps-check', { productDo: rest('pr') }),
  auto('b1-plain', '#b1', '#/workflows/deps-check', { mockDo: toggle, productDo: async (p) => { await toggle(p); await rest('pr')(p); } }),
  auto('c1', '#c1', '#/workflows/pr-evidence', { productDo: rest('may') }),
  auto('c1-plain', '#c1', '#/workflows/pr-evidence', { mockDo: toggle, productDo: async (p) => { await toggle(p); await rest('may')(p); } }),
  auto('c1b', '#c1b', '#/workflows/pr-evidence', { productDo: typeEnd }),
  auto('c1c', '#c1c', '#/workflows/pr-evidence', { productDo: async (p) => { await typeEnd(p); await p.click('.pfoot .btn.primary'); } }),
  box('b2', { inbox: true }),
  box('b3'),
  box('c2', { now: D24C }),
  box('c3', { now: D24C }),
  rules('d1'),
  rules('d2', { productDo: typeRules }),
  rules('d3', { productDo: async (page) => { await typeRules(page); await page.click('.rules-ask button'); await page.waitForSelector('.rules-diff'); await page.waitForSelector('.rules-agent[aria-busy=false]'); } }),
  box('h1', { store: { session: { 'moor.draft.steer:h1': H1DRAFT } } }),
  box('h2'),
  box('h3'),
  box('h4', { now: D25 }),
];
module.exports = { STATES };
