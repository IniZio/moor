import { expect, test } from 'bun:test';

import { type HeadFields, TaskHead } from './TaskHead';
import { all, textOf } from './testkit';

const NOW = new Date(2026, 10, 22, 15, 0).getTime();
const at = new Date(2026, 10, 22, 14, 46).toISOString();

const task = (over: Partial<HeadFields> = {}): HeadFields => ({
	box: 'web/fix-avatar',
	repo: 'acme/web',
	other_repos: [],
	issue: null,
	pr: null,
	title: 'Fix the avatar crop',
	state: 'working',
	stale: false,
	branch: 'moor/fix-avatar',
	machine: null,
	workflow: null,
	last_event_at: at,
	updated_at: at,
	line: 'Running the tests',
	gate: null,
	...over,
});

const head = (over: Partial<HeadFields> = {}) => TaskHead({ t: task(over), now: NOW, actions: <button type="button">Stop</button> });

test('a one-repo task names the repo first, then the title with its state and the actions, then the branch and the exact time', () => {
	const view = head();
	const text = textOf(view);
	expect(textOf(all(view, 'div')[0])).toBe(' acme/web');
	expect(text).not.toContain('starts here');
	expect(textOf(all(view, 'h1')[0])).toBe('Fix the avatar crop');
	expect(textOf(all(view, 'span').find((s) => s.props.class?.includes('t-pill')))).toBe('working');
	expect(all(view, 'button').map(textOf)).toEqual(['Stop']);
	const meta = textOf(all(view, 'p')[0]);
	expect(meta).toBe('moor/fix-avatarlast event 14:46');
});

test('a task in two repos marks the first as where it starts', () => {
	const first = textOf(all(head({ other_repos: ['acme/api'] }), 'div')[0]);
	expect(first).toBe(' acme/webstarts here· acme/api');
});

test('the issue the task is for comes after the repos, then the pull request link', () => {
	const view = head({ issue: 'ENG-412', pr: { repo: 'acme/web', number: 7, url: 'https://example.test/pr/7' } });
	expect(textOf(all(view, 'div')[0])).toBe(' acme/webENG-412PR #7');
	expect(all(view, 'a')[0]?.props.href).toBe('https://example.test/pr/7');
});

test('a task a workflow made says which workflow and pull request, and where it runs; every time is exact', () => {
	const view = head({ machine: 'shared-1', workflow: { name: 'pr-evidence', pull: 'acme/web#7' }, last_event_at: new Date(2026, 10, 20, 9, 5).toISOString() });
	const meta = textOf(all(view, 'p')[0]);
	expect(meta).toBe('moor/fix-avataron shared-1started by pr-evidence · PR #7last event 20 Nov 09:05');
	expect(all(view, 'a').find((a) => textOf(a) === 'pr-evidence')?.props.href).toBe('#/workflows/pr-evidence');
	expect(meta).not.toMatch(/\d+[mhd] ago|now/);
});

test('a task without gates keeps the line saying what it is doing', () => {
	expect(textOf(head())).toContain('Running the tests');
	const gated = textOf(head({ gate: { gates: ['proof'], status: { state: 'open', gate: 'proof', try_n: 1, next: 'agent' } } }));
	expect(gated).not.toContain('Running the tests');
});

test('a one-off task says who asked and when, from the prompt in its timeline; a workflow run keeps its last event', () => {
	const asked = new Date(2026, 10, 22, 14, 14).toISOString();
	const own = TaskHead({ t: task({ machine: 'my-laptop' }), now: NOW, actions: null, asked });
	expect(textOf(all(own, 'p')[0])).toBe('moor/fix-avataron my-laptopasked by you at 14:14');
	const theirs = TaskHead({ t: task({ owner: 'ana@example.com' }), now: NOW, actions: null, asked });
	expect(textOf(all(theirs, 'p')[0])).toBe('moor/fix-avatarasked by ana@example.com at 14:14');
	const flow = TaskHead({ t: task({ workflow: { name: 'nightly' } }), now: NOW, actions: null, asked });
	expect(textOf(all(flow, 'p')[0])).toBe('moor/fix-avatarstarted by nightlylast event 14:46');
});

test('where the pull request stands sits in the first row, after its link', () => {
	const view = TaskHead({ t: task({ pr: { repo: 'acme/web', number: 7, url: 'https://example.test/pr/7' } }), now: NOW, actions: null, pr: <span>● open</span> });
	expect(textOf(all(view, 'div')[0])).toBe(' acme/webPR #7● open');
});
