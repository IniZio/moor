import { expect, test } from 'bun:test';

import type { ReadingView, Workflow } from './api';
import { AutomationBox, type BoxProps, phaseOf } from './AutomationBox';
import { litGates, type Hover } from './promptFile';
import { all, button, type El, textOf } from './testkit';

const SAVED = 'Run the check. Post one comment.';

function box(part: Partial<BoxProps>, calls: string[] = []) {
	const flow = { name: 'pr-evidence', prompt: SAVED, repos: ['acme/web'], cron: null, timezone: null, on_pull_request: true, run_as: 'ana@example.com', may: { allow: { 'github-api': ['POST /repos/{repo}/statuses/*'] } }, revision: 3 } as Workflow;
	const note = (what: string) => () => calls.push(what);
	return AutomationBox({
		flow, prompt: SAVED, read: null, showFile: true, busy: false, problem: null, saved: false,
		onInput: note('input'), onRead: note('read'), onSave: note('save'), onDiscard: note('discard'), onReload: note('reload'), onToggle: note('toggle'),
		...part,
	}) as El;
}

const lit = (el: El) => all(el, 'div').filter((d) => String((d.props as { class?: string }).class ?? '').includes('fadd')).map(textOf);

test('a saved prompt shows its file with the config entries as written, and offers no Save', () => {
	const el = box({});
	expect(textOf(el)).toContain('run_as: ana@example.com');
	expect(textOf(el)).toContain('- POST /repos/{repo}/statuses/*');
	expect(button(el, 'Save')).toBeUndefined();
	expect(button(el, 'Read it')).toBeUndefined();
	expect(button(el, 'Hide file')).toBeDefined();
});

test('while writing, the file keeps the saved prompt and Read it is the next step', () => {
	const calls: string[] = [];
	const el = box({ prompt: SAVED + ' Also post to chat.' }, calls);
	expect(phaseOf(SAVED, SAVED + ' x', null)).toBe('writing');
	expect(textOf(el)).not.toContain('Also post to chat.new');
	expect(button(el, 'Save')).toBeUndefined();
	button(el, 'Read it')?.props.onClick?.();
	expect(calls).toEqual(['read']);
});

test('once read, only the changed line is lit and Save is offered', () => {
	const next = SAVED + ' Also post to chat.';
	const el = box({ prompt: next, read: next });
	expect(lit(el)).toEqual(['Also post to chat.new']);
	expect(button(el, 'Save')).toBeDefined();
	expect(button(el, 'Discard')).toBeDefined();
});

test('typing after Read it goes back to writing', () => {
	expect(phaseOf(SAVED, SAVED + ' Then stop.', SAVED + ' Then')).toBe('writing');
});

test('a stale save offers to load the other change', () => {
	const calls: string[] = [];
	const el = box({ prompt: SAVED + ' x', read: SAVED + ' x', problem: { text: 'moved', stale: true } }, calls);
	button(el, 'Discard mine and load theirs')?.props.onClick?.();
	expect(calls).toEqual(['reload']);
});

test('Hide file leaves the prompt alone', () => {
	const el = box({ showFile: false });
	expect(textOf(el)).not.toContain('run_as:');
	expect(button(el, 'Show file')).toBeDefined();
});

const SRC = 'Run the check. Post one comment on the pull request. Say what you did not run.';
const COMMENT = 'POST /repos/{repo}/issues/*/comments';
const reading = (over: Partial<ReadingView> = {}): ReadingView => ({
	state: 'read',
	stale: [],
	source: SRC,
	runs: [{ text: 'Run the check.', phrase: 'p1' }, { text: ' ' }, { text: 'Post one comment', phrase: 'p2' }, { text: ' on the pull request. Say what you did not run.' }],
	phrases: [
		{ id: 'p1', mark: 'asked', effects: [], intents: [] },
		{ id: 'p2', mark: 'held', effects: ['e1'], intents: [] },
	],
	effects: [{ id: 'e1', kind: 'allow', line: COMMENT, key: 'allow github-api ' + COMMENT, phrases: ['p2'] }],
	gates: null,
	may: { 'github-api': [COMMENT] },
	...over,
});

const FLOW = { name: 'pr-evidence', prompt: SRC, repos: ['acme/web'], cron: null, timezone: null, on_pull_request: true, run_as: 'ana@example.com', may: { allow: { 'github-api': [COMMENT, 'POST /repos/{repo}/statuses/*'] } }, revision: 3 };
const classOf = (el: El) => String((el.props as { class?: string }).class ?? '');
const wordsOf = (el: El) => textOf(all(el, 'div').find((d) => classOf(d).includes('pmarks')));
const phraseSpans = (el: El) => all(el, 'span').filter((s) => (s.props as { 'data-id'?: string })['data-id'] !== undefined);
const spanOf = (el: El, id: string) => phraseSpans(el).find((s) => (s.props as { 'data-id'?: string })['data-id'] === id) as El;
const legendOf = (el: El) => textOf(all(el, 'p').find((p) => classOf(p).includes('legend')));
const litLines = (el: El) => all(el, 'div').filter((d) => classOf(d).includes('flit')).map(textOf);
type Events = El['props'] & Record<'onMouseEnter' | 'onMouseLeave', (() => void) | undefined>;
const fire = (el: El, handler: 'onMouseEnter' | 'onMouseLeave' | 'onFocus') => (el.props as Events)[handler]?.();
/** The box of the saved prompt `prompt` with `read` as the controller's reading of it. */
const marked = (read: ReadingView | null | undefined, part: Partial<BoxProps> = {}, prompt = read?.source ?? SRC) => box({ flow: { ...FLOW, prompt, reading: read } as Workflow, prompt, ...part });

test('hovering a held phrase lights its may line and the prompt line that holds it, and leaving puts them out', () => {
	const seen: Hover[] = [];
	const at = (hover: Hover = null) => marked(reading(), { hover, onHover: (h) => seen.push(h) });
	const rest = at();
	expect(litLines(rest)).toEqual([]);
	expect(classOf(spanOf(rest, 'p2'))).toContain('k-enf');
	fire(spanOf(rest, 'p2'), 'onMouseEnter');
	expect(seen).toEqual([{ phrase: 'p2' }]);
	const over = at({ phrase: 'p2' });
	expect(litLines(over)).toEqual(['    - ' + COMMENT, 'Post one comment on the pull request.']);
	expect(classOf(spanOf(over, 'p2'))).toContain('nlit');
	expect(classOf(spanOf(over, 'p1'))).not.toContain('nlit');
	fire(spanOf(over, 'p2'), 'onMouseLeave');
	expect(seen.at(-1)).toBeNull();
	expect(litLines(at(null))).toEqual([]);
});

test('keyboard focus on a phrase lights what hover does', () => {
	const seen: Hover[] = [];
	const el = marked(reading(), { onHover: (h) => seen.push(h) });
	expect((spanOf(el, 'p2').props as { tabIndex?: number }).tabIndex).toBe(0);
	fire(spanOf(el, 'p2'), 'onFocus');
	const away = { name: 'the next field' };
	(spanOf(el, 'p2').props as { onFocusOut?: (e: { relatedTarget: typeof away; currentTarget: { contains: (n: typeof away) => boolean } }) => void }).onFocusOut?.({ relatedTarget: away, currentTarget: { contains: () => false } });
	expect(seen).toEqual([{ phrase: 'p2' }, null]);
});

test('hovering a file line lights its phrases and only that line', () => {
	const seen: Hover[] = [];
	const rest = marked(reading(), { onHover: (h) => seen.push(h) });
	const line = all(rest, 'div').find((d) => classOf(d).includes('fl') && textOf(d) === '    - ' + COMMENT) as El;
	fire(line, 'onMouseEnter');
	const over = marked(reading(), { hover: seen[0] });
	expect(litLines(over)).toEqual(['    - ' + COMMENT]);
	expect(classOf(spanOf(over, 'p2'))).toContain('nlit');
	expect(classOf(spanOf(over, 'p1'))).not.toContain('nlit');
});

test('an asked phrase is dashed and lights no may line or gate', () => {
	const rest = marked(reading());
	expect(classOf(spanOf(rest, 'p1'))).toContain('k-ask');
	expect(classOf(spanOf(rest, 'p1'))).not.toContain('k-enf');
	expect(litLines(marked(reading(), { hover: { phrase: 'p1' } }))).toEqual(['Run the check.']);
	expect(litGates(reading(), { phrase: 'p1' })).toEqual([]);
});

test('the legend explains the marks only when a reading with phrases is drawn', () => {
	expect(legendOf(marked(reading()))).toBe('Solid: moor holds the agent to it. Dashed: only asked of the agent.');
	expect(legendOf(box({}))).toBe('The file is what each run gets.');
});

test('while writing over a reading, the legend says the marks are off', () => {
	expect(legendOf(marked(reading(), { prompt: SRC + ' Also post to chat.' }, SRC))).toBe('Marks are off while you write.');
});

test('no reading, a reading of no phrases and a failed reading all draw today\'s page', () => {
	const today = textOf(marked(undefined));
	for (const read of [null, reading({ phrases: [], runs: [{ text: SRC }] }), reading({ state: 'failed', failed: 'the quote "Post" was not found', phrases: [], runs: [], source: '' })]) {
		const el = marked(read, {}, SRC);
		expect(textOf(el)).toBe(today);
		expect(phraseSpans(el)).toEqual([]);
		expect(all(el, 'div').some((d) => classOf(d).includes('pmarks'))).toBe(false);
		expect(legendOf(el)).toBe('The file is what each run gets.');
		expect(all(el, 'div').filter((d) => classOf(d).split(' ').includes('fl')).some((d) => (d.props as { onMouseEnter?: unknown }).onMouseEnter !== undefined)).toBe(false);
	}
	expect(all(marked(undefined), 'textarea')).toHaveLength(1);
});

test('a reading of other words than the saved prompt draws no marks', () => {
	const el = marked(reading(), {}, SRC + ' Then stop.');
	expect(phraseSpans(el)).toEqual([]);
	expect(textOf(el)).toContain('run_as: ana@example.com');
});

test('a reading whose runs do not add up to its source draws no marks', () => {
	expect(phraseSpans(marked(reading({ runs: [{ text: 'Run the check.', phrase: 'p1' }] })))).toEqual([]);
});

test('while the draft differs from the reading, the marks are off', () => {
	const el = marked(reading(), { prompt: SRC + ' Also post to chat.' }, SRC);
	expect(phraseSpans(el)).toEqual([]);
	expect(all(el, 'textarea')).toHaveLength(1);
});

test('the marked block says the words of the source in any script', () => {
	const source = '运行检查。发布一条评论，然后停止。';
	const el = marked(reading({ source, runs: [{ text: '运行检查。', phrase: 'p1' }, { text: '发布一条评论', phrase: 'p2' }, { text: '，然后停止。' }] }));
	expect(wordsOf(el)).toBe(source);
	expect(phraseSpans(el).map(textOf)).toEqual(['运行检查。', '发布一条评论']);
});
