import { expect, test } from 'bun:test';

import type { BoxRow, Evidence } from './api';
import { ApproveView } from './Gates';
import type { ApproveProps } from './Gates';
import { type Peekable, peekable, type Wait } from './gate';
import { HomeListView } from './HomeList';
import { peekOf, withQuery } from './hashQuery';
import { handedImages, PeekView } from './Peek';
import { all, button, textOf } from './testkit';

const NOW = new Date(2026, 10, 22, 15, 0).getTime();
const at = new Date(2026, 10, 22, 14, 46).toISOString();

const approval: Wait = { gate: 'review', kind: 'approve', seq: 4, try_n: 1, why: null, handed: ['plan.md', 'after.png'], note: 'Ready for you.', command: null, tail: null, missing: [], hint: null };
const stuck: Wait = { ...approval, kind: 'stuck', handed: [], note: null, why: 'out of tries', says: 'The screenshot shows the new button.', missing: ['after.png'] };

const row = (box: string, over: Partial<BoxRow> = {}): BoxRow => ({
	box,
	repo: 'acme/web',
	other_repos: ['acme/api'],
	title: `Task ${box}`,
	state: 'working',
	stale: false,
	line: 'Running the tests',
	last_event_at: at,
	updated_at: at,
	pr: { repo: 'acme/web', number: 7, url: 'https://example.test/pr/7' },
	failed: false,
	retryable: false,
	...over,
});

const asking = (box: string, gate: Wait) => row(box, { state: 'needs_you', issue: 'https://linear.app/acme/issue/WEB-142/fix-the-avatar', needs: { id: 'ask-1', kind: gate.kind, question: 'q', picks: [], plan: false, gate } }) as Peekable;

const props = (t: Peekable): ApproveProps => ({
	box: t.box,
	wait: t.needs.gate,
	busy: false,
	note: '',
	error: '',
	images: [],
	pr: null,
	onYes: () => {},
	onNo: () => {},
	onNote: () => {},
});

test('the peek of an approval says where the task is, its issue key and what it says, then holds the same card as the task page', () => {
	const t = asking('web/gated', approval);
	const text = textOf(PeekView({ t, now: NOW, approve: props(t), onClose: () => {} }));
	expect(text).toContain('acme/web+1');
	expect(text).toContain('WEB-142');
	expect(text).toContain('Task web/gated');
	expect(text).toContain('14:46');
	expect(text).toContain(textOf(ApproveView(props(t))));
	const view = PeekView({ t, now: NOW, approve: props(t), onClose: () => {} });
	expect(all(view, 'a').find((a) => textOf(a) === 'Open task')?.props.href).toBe('#/box/web%2Fgated');
	expect(button(view, 'Yes, it’s done')).toBeDefined();
	expect(button(view, 'No, send it back')).toBeDefined();
});

test('the peek of a stuck gate holds the stuck card, which sends the person to the task page to write', () => {
	const t = asking('web/stuck', stuck);
	const text = textOf(PeekView({ t, now: NOW, approve: props(t), onClose: () => {} }));
	expect(text).toContain('Stuck on “review”');
	expect(text).toContain('You can write to the agent on the task page, open the terminal and fix it yourself, or give it one more try.');
	expect(text).toContain('after.png');
	expect(text).not.toContain('Yes');
});

test('the close button closes the peek', () => {
	const t = asking('web/gated', approval);
	let closed = 0;
	button(PeekView({ t, now: NOW, approve: props(t), onClose: () => closed++ }), '✕')?.props.onClick?.();
	expect(closed).toBe(1);
});

test('only a task whose gate waits for the person has a peek', () => {
	expect(peekable(asking('a', approval))).toBe(true);
	expect(peekable(row('b'))).toBe(false);
	expect(peekable(row('c', { state: 'needs_you', needs: { id: 'q', kind: 'question', question: 'Which one?', picks: [], plan: false } }))).toBe(false);
	expect(peekable(row('d', { state: 'done' }))).toBe(false);
	const rows = [asking('a', approval), row('b'), row('c', { state: 'done' })];
	const marked = all(HomeListView({ rows, now: NOW, hash: '#/' }), 'a').filter((a) => 'data-row' in a.props);
	const flags = marked.map((a) => [textOf(a).includes('Task a'), (a.props as { 'data-peek'?: string })['data-peek'] !== undefined]);
	expect(flags.filter(([, has]) => has)).toEqual([[true, true]]);
	expect(all(HomeListView({ rows, now: NOW, hash: '#/' }), 'span').filter((s) => 'data-peekbtn' in s.props)).toHaveLength(1);
});

test('the open row is in the address beside the repo and grouping, and picking a repo closes it', () => {
	const hash = withQuery('#/?repo=acme/web&grp=repo', { peek: 'web/gated' });
	expect(hash).toBe('#/?repo=acme/web&grp=repo&peek=web/gated');
	expect(peekOf(hash)).toBe('web/gated');
	expect(peekOf('#/')).toBe('');
	expect(peekOf(withQuery(hash, { peek: '' }))).toBe('');
	const rows = [asking('web/gated', approval), row('b')];
	const links = all(HomeListView({ rows, now: NOW, hash }), 'a').map((a) => a.props.href);
	expect(links).toContain('#/?repo=acme/api&grp=repo');
	expect(links.filter((h) => h?.includes('peek'))).toEqual([]);
});

test('the open row is marked in the list', () => {
	const rows = [asking('web/gated', approval), row('b')];
	const marked = all(HomeListView({ rows, now: NOW, hash: '#/?peek=web/gated' }), 'a').filter((a) => String(a.props.class).includes('t-peeking'));
	expect(marked).toHaveLength(1);
	expect(textOf(marked[0])).toContain('Task web/gated');
});

const file = (name: string, version: number, image: boolean): Evidence => ({ name, version, kind: 'file', content_type: image ? 'image/png' : 'text/plain', size: 1, created_at: at, image, href: `/a/${name}/${version}` });

test('thumbnails are the newest version of each handed-in image, and nothing else', () => {
	const detail = { artifacts: [file('after.png', 1, true), file('after.png', 2, true), file('plan.md', 1, false), file('other.png', 1, true)] };
	expect(handedImages(['plan.md', 'after.png'], detail).map((a) => a.href)).toEqual(['/a/after.png/2']);
	expect(handedImages(['after.png'], null)).toEqual([]);
	const t = asking('web/gated', approval);
	const imgs = all(PeekView({ t, now: NOW, approve: { ...props(t), images: handedImages(t.needs.gate.handed, detail) }, onClose: () => {} }), 'img');
	expect(imgs.map((i) => (i.props as { src?: string }).src)).toEqual(['/a/after.png/2']);
});
