import { expect, test } from 'bun:test';

import type { Node } from './api';
import { blocker, EXAMPLES, type FieldActs, type FieldState, MachineMenuView, TargetSentence, WriteFieldView } from './WriteField';
import { all, button, textOf } from './testkit';

const node = (name: string, scope: Node['scope'], over: Partial<Node> = {}): Node => ({
	name,
	scope,
	runtime: 'docker',
	online: true,
	used: 1,
	waiting: 0,
	max_boxes: 4,
	labels: [],
	revision: 0,
	editable: false,
	...over,
});

const state = (over: Partial<FieldState> = {}): FieldState => ({
	asks: false,
	open: true,
	prompt: '',
	repos: ['acme/web'],
	base: '',
	node: '',
	planFirst: false,
	busy: false,
	error: '',
	menu: '',
	machines: [],
	known: [],
	rows: [],
	now: new Date(2026, 10, 22, 15, 0).getTime(),
	...over,
});

function acts() {
	const calls: string[] = [];
	const a = Object.fromEntries(
		['open', 'example', 'close', 'typed', 'prompt', 'repos', 'base', 'node', 'plan', 'menu', 'start', 'settle'].map((k) => [k, (...x: unknown[]) => calls.push(`${k}:${x.join(',')}`)]),
	) as FieldActs;
	return { a, calls };
}

test('closed, the field is one line that opens as soon as it is focused', () => {
	const { a, calls } = acts();
	const view = WriteFieldView({ s: state({ open: false }), act: a });
	const line = all(view, 'input');
	expect(line).toHaveLength(1);
	expect(line[0].props.placeholder).toBe('Write a task, or paste an issue…');
	expect(all(view, 'textarea')).toHaveLength(0);
	expect(button(view, 'Start')).toBeUndefined();
	line[0].props.onFocus?.();
	expect(calls).toEqual(['open:']);
});

test('open, it is the task form: editor, sentence and Start, which says what is missing instead of greying out', () => {
	const { a } = acts();
	const empty = WriteFieldView({ s: state(), act: a });
	expect(all(empty, 'textarea')).toHaveLength(1);
	expect(textOf(empty)).toContain('in acme/web');
	const start = (v: ReturnType<typeof WriteFieldView>) => all(v, 'button').find((b) => b.props.type === 'submit');
	expect(start(empty)?.props.disabled).toBe(false);
	expect(blocker({ prompt: ' ', repos: ['acme/web'] })).toBe('Write what the agent should do first.');
	expect(blocker({ prompt: 'Fix the bug', repos: [] })).toBe('Pick a repo first.');
	expect(blocker({ prompt: 'Fix the bug', repos: ['acme/web'] })).toBeNull();
	expect(start(WriteFieldView({ s: state({ busy: true }), act: a }))?.props.disabled).toBe(true);
});

test('an example link fills the editor with that example', () => {
	const { a, calls } = acts();
	const view = WriteFieldView({ s: state(), act: a });
	expect(textOf(view)).toContain('Try an example:');
	button(view, EXAMPLES.pr.label)?.props.onClick?.();
	expect(calls).toEqual(['example:pr']);
});

test('the sentence names one repo, the base and the machine, with no note', () => {
	const { a } = acts();
	const view = WriteFieldView({ s: state({ base: 'main' }), act: a });
	expect(textOf(view)).toContain('in acme/web · from  · on any free machine');
	expect(all(view, 'input').some((i) => i.props.value === 'main')).toBe(true);
	expect(textOf(view)).not.toContain('cloned beside');
});

test('with two repos the sentence joins them and one line says where the agent starts', () => {
	const { a } = acts();
	const view = WriteFieldView({ s: state({ repos: ['acme/shop-api', 'acme/shop-web'] }), act: a });
	const text = textOf(view);
	expect(text).toContain('in acme/shop-api + acme/shop-web');
	expect(text).toContain('The agent starts in acme/shop-api. The others are cloned beside it.');
});

test('the repo and machine words open their menus', () => {
	const { a, calls } = acts();
	const sentence = TargetSentence({ s: state(), act: a });
	const words = all(sentence, 'button');
	words.find((b) => textOf(b) === 'acme/web')?.props.onClick?.();
	words.find((b) => textOf(b) === 'any free machine')?.props.onClick?.();
	expect(calls).toEqual(['menu:repo', 'menu:machine']);
});

test('the machine menu lists shared machines and yours, and an offline one is disabled with its time', () => {
	const picked: string[] = [];
	const down = Math.floor(new Date(2026, 10, 21, 8, 5).getTime() / 1000);
	const view = MachineMenuView({
		nodes: [node('shared-1', 'shared'), node('laptop', 'personal', { online: false, offline_since: down }), node('desk', 'personal')],
		value: '',
		now: new Date(2026, 10, 22, 15, 0).getTime(),
		onPick: (n) => picked.push(n),
		onDone: () => undefined,
	});
	const items = all(view, 'button');
	const laptop = items.find((b) => textOf(b).startsWith('laptop'));
	expect(laptop?.props.disabled).toBe(true);
	expect(textOf(laptop)).toContain('offline since 21 Nov 08:05');
	items.find((b) => textOf(b).startsWith('desk'))?.props.onClick?.();
	items.find((b) => textOf(b).startsWith('any free machine'))?.props.onClick?.();
	expect(picked).toEqual(['desk', '']);
	expect(textOf(view)).toContain('3 free');
	expect(['Shared', 'Yours'].every((h) => textOf(view).includes(h))).toBe(true);
});

test('for a person whose sends are questions the form asks: no base, machine or plan, and the button says Ask', () => {
	const { a } = acts();
	const view = WriteFieldView({ s: state({ asks: true, prompt: 'Where is GET /items handled?' }), act: a });
	const text = textOf(view);
	expect(text).toContain('in acme/web');
	expect(text).not.toMatch(/from|machine|Plan first/);
	expect(all(view, 'input').some((i) => i.props.placeholder === 'default branch')).toBe(false);
	const submit = all(view, 'button').find((b) => b.props.type === 'submit');
	expect(textOf(submit)).toBe('Ask');
	expect(textOf(all(WriteFieldView({ s: state({ asks: true, busy: true }), act: a }), 'button').find((b) => b.props.type === 'submit'))).toBe('Asking…');
});

test('for everyone else the form is the task form it always was', () => {
	const { a } = acts();
	const view = WriteFieldView({ s: state(), act: a });
	expect(textOf(view)).toContain('Plan first');
	expect(textOf(all(view, 'button').find((b) => b.props.type === 'submit'))).toBe('Start');
});
