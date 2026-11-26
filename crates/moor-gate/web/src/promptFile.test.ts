import { expect, test } from 'bun:test';

import type { ReadingView, Workflow } from './api';
import { changed, fileLines, litGates, litOf, marksOf, mayLines, onLine, promptLines } from './promptFile';

const PROMPT = 'Check out the pull request and run the check. Take screenshots.\n\n- Post one comment\nSay what you did not run';

function flow(part: Partial<Workflow>): Workflow {
	return { cron: null, timezone: null, on_pull_request: false, repos: ['acme/web', 'acme/api'], run_as: 'ana@example.com', may: { allow: {} }, prompt: PROMPT, ...part } as Workflow;
}

test('the prompt splits a sentence a line and loses no text', () => {
	const lines = promptLines(PROMPT);
	expect(lines).toEqual(['Check out the pull request and run the check.', 'Take screenshots.', '', '- Post one comment', 'Say what you did not run']);
	expect(lines.join(' ').replace(/\s+/g, ' ')).toBe(PROMPT.replace(/\s+/g, ' '));
});

test('the on line says what fires the automation', () => {
	expect(onLine(flow({ cron: '0 9 * * 1-5', timezone: 'Asia/Hong_Kong' }))).toBe('on: {schedule: "0 9 * * 1-5", timezone: Asia/Hong_Kong}');
	expect(onLine(flow({ on_pull_request: true }))).toBe('on: {pull_request: [opened, reopened, ready_for_review, synchronize]}');
	expect(onLine(flow({ cron: '0 9 * * *', on_pull_request: true }))).toContain('schedule: "0 9 * * *", pull_request');
	expect(onLine(flow({}))).toContain('only when started by hand');
});

test('may shows the config entries as written, or why the config does not parse', () => {
	const text = mayLines({ allow: { 'github-api': ['POST /repos/{repo}/issues/*/comments'] } }).map((l) => l.text);
	expect(text).toEqual(['may:', '  github-api:', '    - POST /repos/{repo}/issues/*/comments']);
	expect(mayLines({ allow: {} })[0].text).toBe('may: {}');
	expect(mayLines({ error: 'proxy.allow.nope: only a default rule' }).map((l) => l.text).join('\n')).toContain('proxy.allow.nope');
});

test('one added sentence lights one line', () => {
	expect(changed(['a.', 'b.', 'c.'], ['a.', 'b.', 'new.', 'c.'])).toEqual([false, false, true, false]);
	expect(changed(['a.', 'b.'], ['a.', 'B.'])).toEqual([false, true]);
});

test('the file reads on, repos, run_as and may, then the prompt, with only the edited lines lit', () => {
	const saved = fileLines(flow({}), PROMPT);
	expect(saved.slice(0, 4).map((l) => l.text)).toEqual(['on: {}    # only when started by hand', 'repos: [acme/web, acme/api]', 'run_as: ana@example.com', 'may: {}']);
	expect(saved.some((l) => l.lit)).toBe(false);
	const edited = fileLines(flow({}), PROMPT + '\nAlso post the summary to the team chat.');
	expect(edited.filter((l) => l.lit).map((l) => l.text)).toEqual(['Also post the summary to the team chat.']);
});


const SRC = 'Run the check. Post one comment on the pull request. Say what you did not run.';
const COMMENT = 'POST /repos/{repo}/issues/*/comments';
const runs = [{ text: 'Run the check.', phrase: 'p1' }, { text: ' ' }, { text: 'Post one comment', phrase: 'p2' }, { text: ' on the pull request. Say what you did not run.' }];
const read = (over: Partial<ReadingView> = {}): ReadingView => ({
	state: 'read',
	stale: [],
	source: SRC,
	runs,
	phrases: [
		{ id: 'p1', mark: 'asked', effects: [] },
		{ id: 'p2', mark: 'held', effects: ['e1', 'e2'] },
	],
	effects: [
		{ id: 'e1', kind: 'allow', line: COMMENT, key: 'allow ' + COMMENT, phrases: ['p2'] },
		{ id: 'e2', kind: 'gate', line: 'comment', key: 'gate comment', phrases: ['p2'] },
	],
	gates: null,
	may: {},
	...over,
});

test('marks are drawn only for a read reading with phrases whose runs are the prompt', () => {
	expect(marksOf(read(), SRC)).not.toBeNull();
	expect(marksOf(null, SRC)).toBeNull();
	expect(marksOf(undefined, SRC)).toBeNull();
	expect(marksOf(read({ state: 'failed', failed: 'quote not found' }), SRC)).toBeNull();
	expect(marksOf(read({ state: 'reading' }), SRC)).toBeNull();
	expect(marksOf(read({ phrases: [], runs: [{ text: SRC }] }), SRC)).toBeNull();
	expect(marksOf(read(), SRC + ' Then stop.')).toBeNull();
	expect(marksOf(read({ runs: [{ text: 'Run the check.', phrase: 'p1' }] }), SRC)).toBeNull();
});

test('the file lists the phrases of each line: an allow line by its effect, a prompt line by the words on it', () => {
	const f = flow({ prompt: SRC, may: { allow: { 'github-api': [COMMENT] } }, reading: read() });
	const at = (text: string) => fileLines(f, SRC).find((l) => l.text.trim() === text)?.phrases;
	expect(at('- ' + COMMENT)).toEqual(['p2']);
	expect(at('Run the check.')).toEqual(['p1']);
	expect(at('Post one comment on the pull request.')).toEqual(['p2']);
	expect(at('Say what you did not run.')).toEqual([]);
	expect(fileLines(f, SRC).find((l) => l.kind === 'head' && l.text.startsWith('on:'))?.phrases).toEqual([]);
});

test('a phrase over two sentences is on both lines, and a script with no sentence stop is one line', () => {
	const two = read({ runs: [{ text: 'Run the check. Post one comment', phrase: 'p1' }, { text: ' on the pull request. Say what you did not run.' }], phrases: [{ id: 'p1', mark: 'asked', effects: [] }], effects: [] });
	const lines = fileLines(flow({ prompt: SRC, reading: two }), SRC).filter((l) => l.kind === 'prompt');
	expect(lines.map((l) => l.phrases)).toEqual([['p1'], ['p1'], []]);
	const cjk = '运行检查，发布一条评论';
	const one = read({ source: cjk, runs: [{ text: '运行检查', phrase: 'p1' }, { text: '，' }, { text: '发布一条评论', phrase: 'p2' }] });
	expect(fileLines(flow({ prompt: cjk, reading: one }), cjk).filter((l) => l.kind === 'prompt').map((l) => l.phrases)).toEqual([['p1', 'p2']]);
});

test('with no reading, no line lists phrases', () => {
	expect(fileLines(flow({}), PROMPT).some((l) => l.phrases !== undefined)).toBe(false);
});

test('hovering a phrase lights the lines that list it, and a line lights its phrases and itself', () => {
	const f = flow({ prompt: SRC, may: { allow: { 'github-api': [COMMENT] } }, reading: read() });
	const lines = fileLines(f, SRC);
	const index = (text: string) => lines.findIndex((l) => l.text.trim() === text);
	const phrase = litOf(lines, { phrase: 'p2' });
	expect([...phrase.phrases]).toEqual(['p2']);
	expect([...phrase.lines].sort()).toEqual([index('- ' + COMMENT), index('Post one comment on the pull request.')].sort());
	const line = litOf(lines, { line: index('- ' + COMMENT) });
	expect([...line.phrases]).toEqual(['p2']);
	expect([...line.lines]).toEqual([index('- ' + COMMENT)]);
	expect(litOf(lines, null).phrases.size + litOf(lines, null).lines.size).toBe(0);
});

test('hovering a phrase names the gates it produced, and an asked phrase names none', () => {
	expect(litGates(read(), { phrase: 'p2' })).toEqual(['comment']);
	expect(litGates(read(), { phrase: 'p1' })).toEqual([]);
	expect(litGates(read(), { line: 3 })).toEqual([]);
	expect(litGates(null, { phrase: 'p2' })).toEqual([]);
});
