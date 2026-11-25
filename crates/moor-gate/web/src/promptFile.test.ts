import { expect, test } from 'bun:test';

import type { Workflow } from './api';
import { changed, fileLines, mayLines, onLine, promptLines } from './promptFile';

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
