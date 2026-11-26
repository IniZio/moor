import type { Workflow } from './api';
import { diffLines } from './rulesDiff';

/** One line of the automation's file: a header line, a comment, the separator, or a line of the prompt. */
export type FileLine = { text: string; kind: 'head' | 'comment' | 'sep' | 'prompt'; lit: boolean };

/** What fires a pull request run: the pull request actions moor acts on. */
const PULL_ACTIONS = '[opened, reopened, ready_for_review, synchronize]';

/** The `on:` line, from the workflow's cron, zone and pull request switch. */
export function onLine(flow: Pick<Workflow, 'cron' | 'timezone' | 'on_pull_request'>): string {
	const parts: string[] = [];
	if (flow.cron != null) parts.push(`schedule: "${flow.cron}"`);
	if (flow.cron != null && flow.timezone) parts.push(`timezone: ${flow.timezone}`);
	if (flow.on_pull_request) parts.push(`pull_request: ${PULL_ACTIONS}`);
	return parts.length === 0 ? 'on: {}    # only when started by hand' : `on: {${parts.join(', ')}}`;
}

/** The `may:` lines: the config's extra Proxy entries under their rule, as written. */
export function mayLines(may: Workflow['may']): FileLine[] {
	const head = (text: string): FileLine => ({ text, kind: 'head', lit: false });
	const note = (text: string): FileLine => ({ text, kind: 'comment', lit: false });
	if (may.error) return [head('may:'), note(`  # the config does not parse: ${may.error}`)];
	const rules = Object.entries(may.allow ?? {});
	if (rules.length === 0) return [head('may: {}'), note("  # the repo's rules, nothing more")];
	return [head('may:'), ...rules.flatMap(([rule, entries]) => [head(`  ${rule}:`), ...entries.map((e) => head(`    - ${e}`))])];
}

/** The prompt one sentence a line, losing no text: lines stay lines, and a line splits after each sentence end. */
export function promptLines(prompt: string): string[] {
	return prompt
		.trimEnd()
		.split('\n')
		.flatMap((line) => (line.trim() === '' ? [''] : line.split(/(?<=[.!?])\s+(?=\S)/)));
}

/** Which of `next` are not in a longest common run with `prev`: the lines a change added or rewrote. */
export function changed(prev: string[], next: string[]): boolean[] {
	return diffLines(prev, next)
		.filter((l) => l.kind !== 'del')
		.map((l) => l.kind === 'add');
}

/** The whole file: `on`, `repos`, `run_as`, `may`, then the prompt, its lines lit where `prompt` differs from the saved one. */
export function fileLines(flow: Workflow, prompt: string): FileLine[] {
	const next = promptLines(prompt);
	const lit = prompt === flow.prompt ? next.map(() => false) : changed(promptLines(flow.prompt), next);
	return [
		{ text: onLine(flow), kind: 'head', lit: false },
		{ text: `repos: [${flow.repos.join(', ')}]`, kind: 'head', lit: false },
		{ text: `run_as: ${flow.run_as}`, kind: 'head', lit: false },
		...mayLines(flow.may),
		{ text: '---', kind: 'sep', lit: false },
		...next.map((text, k) => ({ text, kind: 'prompt' as const, lit: lit[k] && text !== '' })),
	];
}
