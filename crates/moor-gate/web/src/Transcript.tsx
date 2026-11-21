import { useState } from 'preact/hooks';

import type { Clipped, Entry, Tool } from './api';
import { plain } from './gate';

/** Lines an input, diff or output shows before Show all. */
const SHOW_LINES = 12;

/** A routine tool call, which folds with the ones beside it; a failed one never does. */
export function routine(e: Entry): boolean {
	return !!e.tool?.group && !e.tool.failed;
}

/** The words for a run of routine calls, such as "Read 4 files, searched 2 times". */
export function foldWords(entries: Entry[]): string {
	const n = { read: 0, search: 0, plan: 0 };
	for (const e of entries) if (e.tool?.group) n[e.tool.group] += 1;
	const many = (k: number, one: string, more: string) => `${k} ${k === 1 ? one : more}`;
	const parts = [
		n.read && `read ${many(n.read, 'file', 'files')}`,
		n.search && `searched ${many(n.search, 'time', 'times')}`,
		n.plan && (n.plan === 1 ? 'updated the plan' : `updated the plan ${n.plan} times`),
	].filter(Boolean);
	const said = parts.join(', ');
	return said.charAt(0).toUpperCase() + said.slice(1);
}

function lineTone(line: string): string {
	if (line.startsWith('@@')) return 't-tl t-tl-hunk';
	if (line.startsWith('+')) return 't-tl t-tl-add';
	if (line.startsWith('-')) return 't-tl t-tl-del';
	return 't-tl';
}

/** A text moor kept, its first lines shown and the rest behind Show all. */
function Clip({ label, body, diff }: { label: string; body: Clipped; diff?: boolean }) {
	const [all, setAll] = useState(false);
	const lines = body.text.replace(/\n$/, '').split('\n');
	const shown = all ? lines : lines.slice(0, SHOW_LINES);
	return (
		<div class="t-clip">
			<div class="t-clip-h">{label}</div>
			<pre class={diff ? 't-clip-text t-clip-diff' : 't-clip-text'}>
				{diff
					? shown.map((line, i) => (
							<span class={lineTone(line)} key={i}>
								{line || ' '}
							</span>
						))
					: shown.join('\n')}
			</pre>
			{lines.length > SHOW_LINES && (
				<button type="button" class="t-clip-all" onClick={() => setAll(!all)}>
					{all ? 'Show less' : `Show all ${lines.length} lines`}
				</button>
			)}
			{body.cut && (all || lines.length <= SHOW_LINES) && <p class="t-clip-note">moor kept only part of this. The Terminal has the rest.</p>}
		</div>
	);
}

function inputLabel(t: Tool): string {
	switch (t.name) {
		case 'Bash':
			return 'Command';
		case 'Grep':
		case 'Glob':
			return 'Search';
		case 'Agent':
			return 'Prompt';
		case 'TodoWrite':
			return 'Plan';
		default:
			return 'Input';
	}
}

/** A report moor did not accept: a plain item with moor's reason, and what the agent sent folded away. */
function RefusedLine({ t, when }: { t: Tool; when: string }) {
	return (
		<div class="t-ev t-refused">
			<span class="t-who">Agent</span>
			<span class="t-when">{when}</span>
			<div class="t-what">
				<p class="t-refused-title">{t.title}</p>
				{t.output && <p class="t-refused-why">{plain(t.output.text)}</p>}
				{t.input && (
					<details class="t-refused-sent">
						<summary>What the agent sent</summary>
						<pre class="t-quote">{t.input.text}</pre>
					</details>
				)}
			</div>
		</div>
	);
}

/** One tool call: a line that opens to its input, diff and output. A failed call stays open. */
export function ToolLine({ e, when }: { e: Entry; when: string }) {
	const t = e.tool;
	if (!t) return null;
	if (t.refused) return <RefusedLine t={t} when={when} />;
	return (
		<details class={`t-tool${t.failed ? ' t-tool-err' : ''}`} open={t.failed}>
			<summary>
				<span class="t-who">{t.failed ? 'Error' : 'Agent'}</span>
				<span class="t-when">{when}</span>
				<span class="t-tool-title">
					<span class="t-chev">▸</span> <span class="t-mono">{t.title}</span>
				</span>
			</summary>
			<div class="t-tool-body">
				{t.input && <Clip label={inputLabel(t)} body={t.input} />}
				{t.diff && <Clip label="Change" body={t.diff} diff />}
				{t.output && <Clip label={t.failed ? 'What it said' : 'Output'} body={t.output} />}
				{!t.input && !t.diff && !t.output && <p class="t-clip-note">moor kept no more of this call.</p>}
			</div>
		</details>
	);
}

/** Routine calls run together, as one line that opens to each. */
export function ToolFold({ entries, when }: { entries: Entry[]; when: (iso: string) => string }) {
	return (
		<details class="t-tool t-tool-fold">
			<summary>
				<span class="t-who">Agent</span>
				<span class="t-when">{when(entries[0]?.at ?? '')}</span>
				<span class="t-tool-title">
					<span class="t-chev">▸</span> {foldWords(entries)}
				</span>
			</summary>
			<div class="t-tool-runs">
				{entries.map((e) => (
					<ToolLine e={e} when={when(e.at)} key={e.key ?? e.id} />
				))}
			</div>
		</details>
	);
}
