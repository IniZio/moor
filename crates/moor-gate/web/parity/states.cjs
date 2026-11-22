'use strict';
const click = (sel) => (page) => page.click(sel);
const STATES = [
  { key: 'home', mock: '#/tasks', product: '#/', ready: '.t-row' },
  { key: 'home-everyone', mock: '#/tasks?who=all', product: '#/?who=everyone', ready: '.t-row' },
  { key: 'home-repo-filter', mock: '#/tasks?pick=repo', product: '#/', ready: '.t-row', productDo: click('.t-rmenu summary') },
  { key: 'home-grouped', mock: '#/tasks?grp=repo', product: '#/?grp=repo', ready: '.t-row' },
  { key: 'write-open', mock: '#/tasks?write=1', product: '#/new', ready: '.t-wform' },
  { key: 'write-repo-menu', mock: '#/tasks?write=1&pick=r', product: '#/new', ready: '.t-wform', productDo: click('.t-wform [aria-haspopup=dialog]') },
  { key: 'peek-yes', mock: '#/tasks?peek=41', product: '#/?peek=mock-41', ready: '.t-row' },
  { key: 'peek-stuck', mock: '#/tasks?peek=43', product: '#/?peek=mock-43', ready: '.t-row' },
  { key: 'inbox', mock: '#/inbox', product: '#/', ready: '.t-row', productDo: (page) => page.getByRole('button', { name: /Inbox/ }).click() },
  { key: 'task-yes', mock: '#/task/41?scene=yes', product: '#/box/mock-41', ready: 'main.app-box h1, main.app-box h2' },
  { key: 'task-stuck', mock: '#/task/43?scene=stuck', product: '#/box/mock-43', ready: 'main.app-box h1, main.app-box h2' },
  { key: 'task-working', mock: '#/task/44?scene=working', product: '#/box/mock-44', ready: 'main.app-box h1, main.app-box h2' },
  { key: 'task-fixed', mock: '#/task/40?scene=fixed', product: '#/box/mock-40', ready: 'main.app-box h1, main.app-box h2' },
];
module.exports = { STATES };
