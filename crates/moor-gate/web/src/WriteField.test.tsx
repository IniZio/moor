import { expect, test } from 'bun:test';

import { blocker, EXAMPLES, type FieldActs, type FieldState, TargetSentence, WriteFieldView } from './WriteField';
import { all, button, textOf } from './testkit';

const state = (over: Partial<FieldState> = {}): FieldState => ({
	open: true,
	prompt: '',
	repos: ['acme/web'],
	busy: false,
	error: '',
	menu: '',
	known: [],
	rows: [],
	...over,
});

function acts() {
	const calls: string[] = [];
	const a = Object.fromEntries(
		['open', 'example', 'close', 'typed', 'prompt', 'repos', 'menu', 'start', 'settle'].map((k) => [k, (...x: unknown[]) => calls.push(`${k}:${x.join(',')}`)]),
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

test('the sentence names the repo and nothing else to pick, with no note', () => {
	const { a } = acts();
	const view = WriteFieldView({ s: state(), act: a });
	const sentence = all(view, 'p').find((p) => p.props.class === 't-target');
	expect(textOf(sentence)).toBe('in acme/web');
	expect(all(view, 'input')).toHaveLength(0);
	expect(textOf(view)).not.toContain('cloned beside');
	expect(textOf(view)).not.toContain('any free machine');
	expect(textOf(view)).not.toContain('Plan first');
});

test('with two repos the sentence joins them and one line says where the agent starts', () => {
	const { a } = acts();
	const view = WriteFieldView({ s: state({ repos: ['acme/shop-api', 'acme/shop-web'] }), act: a });
	const text = textOf(view);
	expect(text).toContain('in acme/shop-api + acme/shop-web');
	expect(text).toContain('The agent starts in acme/shop-api. The others are cloned beside it.');
});

test('the repo word opens its menu', () => {
	const { a, calls } = acts();
	const sentence = TargetSentence({ s: state(), act: a });
	all(sentence, 'button')
		.find((b) => textOf(b) === 'acme/web')
		?.props.onClick?.();
	expect(calls).toEqual(['menu:repo']);
});

test('the form is the task form, with Start', () => {
	const { a } = acts();
	const view = WriteFieldView({ s: state(), act: a });
	expect(textOf(all(view, 'button').find((b) => b.props.type === 'submit'))).toBe('Start');
});
