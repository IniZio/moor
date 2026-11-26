import type { ReadingView, Workflow } from './api';
import { diffLines } from './rulesDiff';

/** One line of the automation's file: a header line, a comment, the separator, or a line of the prompt. */
export type FileLine = {
	text: string;
	kind: 'head' | 'comment' | 'sep' | 'prompt';
	lit: boolean;
	/** The phrases of the reading that this line holds or lists; set only while marks are drawn. */
	phrases?: string[];
};

/** What the pointer or keyboard focus is on while marks are drawn: a phrase, or a file line by its place in the file. */
export type Hover = { phrase: string } | { line: number } | null;

/** The reading to draw over `prompt`: one that was read, has a solid or dashed phrase, and whose runs say the prompt word for word. */
export function marksOf(reading: ReadingView | null | undefined, prompt: string): ReadingView | null {
	if (reading?.state !== 'read' || !reading.phrases.some((p) => p.mark === 'held' || p.mark === 'asked')) return null;
	return reading.source === prompt && reading.runs.map((r) => r.text).join('') === prompt ? reading : null;
}

const entryOf = (text: string) => text.trim().replace(/^-\s+/, '');
const unique = (ids: string[]) => [...new Set(ids)];

/** Each line with the phrases it holds: an allow line by the effects that name it, a line of the prompt by the runs of phrases that fall on it. */
function withPhrases(lines: FileLine[], reading: ReadingView): FileLine[] {
	let at = 0;
	const spans = reading.runs.map((r) => {
		const span = { id: r.phrase, from: at, to: at + r.text.length };
		at = span.to;
		return span;
	});
	let cursor = 0;
	return lines.map((l) => {
		if (l.kind === 'head' && l.text.trim().startsWith('- ')) {
			const effects = reading.effects.filter((e) => e.kind === 'allow' && entryOf(e.line) === entryOf(l.text));
			return { ...l, phrases: unique(effects.flatMap((e) => e.phrases)) };
		}
		if (l.kind !== 'prompt' || l.text === '') return { ...l, phrases: [] };
		const from = reading.source.indexOf(l.text, cursor);
		if (from < 0) return { ...l, phrases: [] };
		cursor = from + l.text.length;
		return { ...l, phrases: unique(spans.flatMap((s) => (s.id && s.from < cursor && s.to > from ? [s.id] : []))) };
	});
}

export type Lit = { phrases: ReadonlySet<string>; lines: ReadonlySet<number> };

const none: Lit = { phrases: new Set(), lines: new Set() };

/** What lights while the pointer is on `hover`: a phrase lights the lines that hold it, a line lights its phrases and itself. */
export function litOf(lines: readonly FileLine[], hover: Hover): Lit {
	if (hover === null) return none;
	if ('phrase' in hover) return { phrases: new Set([hover.phrase]), lines: new Set(lines.flatMap((l, i) => (l.phrases?.includes(hover.phrase) ? [i] : []))) };
	return { phrases: new Set(lines[hover.line]?.phrases ?? []), lines: new Set([hover.line]) };
}

/** The names of the gate rows lit while the pointer is on a phrase: the gates that phrase produced. */
export function litGates(reading: ReadingView | null | undefined, hover: Hover): string[] {
	if (!reading || hover === null || !('phrase' in hover)) return [];
	const ids = reading.phrases.find((p) => p.id === hover.phrase)?.effects ?? [];
	return reading.effects.filter((e) => e.kind === 'gate' && ids.includes(e.id)).map((e) => e.line);
}

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
	const lines: FileLine[] = [
		{ text: onLine(flow), kind: 'head', lit: false },
		{ text: `repos: [${flow.repos.join(', ')}]`, kind: 'head', lit: false },
		{ text: `run_as: ${flow.run_as}`, kind: 'head', lit: false },
		...mayLines(flow.may),
		{ text: '---', kind: 'sep', lit: false },
		...next.map((text, k) => ({ text, kind: 'prompt' as const, lit: lit[k] && text !== '' })),
	];
	const marks = marksOf(flow.reading, prompt);
	return marks ? withPhrases(lines, marks) : lines;
}
