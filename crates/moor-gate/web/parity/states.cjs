'use strict';
const click = (sel) => (page) => page.click(sel);
// What the product cannot show yet, hidden on the mock before measuring, each with why.
const PHRASE = { sel: '.glx .phr', why: 'the prompt phrase each gate quotes needs the prompt reader (spec #1628)' };
const ENDS = { sel: '.gl.gend:not(.ended)', why: 'a one-off task\'s outcomes: the gate view carries none, only a workflow does' };
const TASK = { mock: [PHRASE, ENDS] };
const FIXED = { mock: [PHRASE, ENDS], product: [{ sel: '.t-th1 .t-stop', why: 'Stop on a done task: the agent still runs, so the product keeps Stop (Ending.tsx STOPPABLE); the mock hides it' }] };
const WORKING = { mock: [PHRASE, ENDS, { sel: '.gl.hand .glx', why: "a working gate's rule: a one-off task carries no gate docs, only the wait of a gate that asks" }] };
const INBOX = {
  mock: [
    { sel: '.iitem:has(.irow.read)', why: 'done tasks in the inbox: the inbox lists only what needed the person, and no spec PR adds a finished task yet' },
  ],
};
const STATES = [
  { key: 'home', mock: '#/tasks', product: '#/', ready: '.t-row' },
  { key: 'home-everyone', mock: '#/tasks?who=all', product: '#/?who=everyone', ready: '.t-row' },
  { key: 'home-repo-filter', mock: '#/tasks?pick=repo', product: '#/', ready: '.t-row', productDo: click('.t-rmenu summary') },
  { key: 'home-grouped', mock: '#/tasks?grp=repo', product: '#/?grp=repo', ready: '.t-row' },
  { key: 'write-open', mock: '#/tasks?write=1', product: '#/new', ready: '.t-wform' },
  { key: 'write-repo-menu', mock: '#/tasks?write=1&pick=r', product: '#/new', ready: '.t-wform', productDo: click('.t-wform [aria-haspopup=dialog]') },
  { key: 'peek-yes', mock: '#/tasks?peek=41', product: '#/?peek=mock-41', ready: '.t-row' },
  { key: 'peek-stuck', mock: '#/tasks?peek=43', product: '#/?peek=mock-43', ready: '.t-row' },
  { key: 'inbox', mock: '#/inbox', product: '#/', ready: '.t-row', productDo: (page) => page.getByRole('button', { name: /Inbox/ }).click(), mask: INBOX },
  { key: 'task-yes', mock: '#/task/41?scene=yes', product: '#/box/mock-41', ready: 'main.app-box h1, main.app-box h2', mask: TASK },
  { key: 'task-stuck', mock: '#/task/43?scene=stuck', product: '#/box/mock-43', ready: 'main.app-box h1, main.app-box h2', mask: TASK },
  { key: 'task-working', mock: '#/task/44?scene=working', product: '#/box/mock-44', ready: 'main.app-box h1, main.app-box h2', mask: WORKING },
  { key: 'task-fixed', mock: '#/task/40?scene=fixed', product: '#/box/mock-40', ready: 'main.app-box h1, main.app-box h2', mask: FIXED },
];
module.exports = { STATES };
