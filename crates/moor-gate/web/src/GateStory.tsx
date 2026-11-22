import type { ComponentChildren } from 'preact';

import type { Workflow } from './api';
import { closingRow, GATE_LOOK, type GateRow, type GateView, gateRows, ruleSentence, type Wait } from './gate';
import './story.css';

/** The states in which a gate is in hand. */
const IN_HAND = new Set<GateRow['state']>(['working', 'asking', 'stuck', 'nocheck', 'failed']);

type Docs = Pick<Workflow, 'gates' | 'outcomes'>;

/** A gate's rule starts a sentence, except where it starts with the name moor. */
const sentence = (rule: string) => (rule.startsWith('moor') ? rule : rule.charAt(0).toUpperCase() + rule.slice(1));

function Row({ look, hand, open, body, summary, end }: { look: GateRow['state']; hand: boolean; open: boolean; body: ComponentChildren; summary: ComponentChildren; end?: boolean }) {
	const { glyph, tone } = GATE_LOOK[look];
	const head = (
		<>
			<span class="t-sg-mark" aria-hidden="true">
				{glyph}
			</span>
			{summary}
		</>
	);
	return (
		<li class={`t-sg t-${tone}${hand ? ' t-sg-hand' : ''}${end ? ' t-sg-end' : ''}`}>
			{body ? (
				<details open={open}>
					<summary class="t-sg-row">{head}</summary>
					<div class="t-sg-body">{body}</div>
				</details>
			) : (
				<div class="t-sg-row">{head}</div>
			)}
		</li>
	);
}

/** How the run ended, once it did; else the outcome moor is deciding, else the endings the workflow lists. */
function outcomeRow(gate: GateView, docs: Docs | null): { look: GateRow['state']; hand: boolean; open: boolean; line: ComponentChildren; says?: string | null } | null {
	const { status } = gate;
	if (status.state === 'passed') {
		const o = status.outcome;
		if (!o) return null;
		return { look: o.tone === 'good' ? 'passed' : o.tone, hand: false, open: true, line: (
			<span>
				ended as: <b>{o.name}</b>
			</span>
		), says: o.says };
	}
	const closing = closingRow(gate);
	if (closing) return { look: closing.state, hand: IN_HAND.has(closing.state), open: IN_HAND.has(closing.state), line: <><span class="t-sg-name">outcome</span><span class="t-sg-state">{closing.words}</span></> };
	const names = (docs?.outcomes ?? []).map((o) => (o.anytime ? `${o.name} (at any point)` : o.name));
	if (names.length === 0 || gate.run_ended) return null;
	return { look: 'later', hand: false, open: false, line: <span>ends as: {names.join(' · ')}</span> };
}

/** What a gate that waits for a person holds the agent to, read off the wait when no workflow describes the gate. */
function waitRule(wait: Wait | null | undefined): string {
	if (wait?.kind === 'approve') return 'needs your yes before it counts as done';
	return wait?.says ? `moor checks: ${wait.says.replace(/\.$/, '')}` : '';
}

/** One row per gate with its mark, number, name and state; the gate in hand is open, with what must be true and the card in place. Ends with the outcome. `wait` is the gate's wait when it waits for a person. */
export function GateStory({ gate, docs, card, wait }: { gate: GateView; docs: Docs | null; card: ComponentChildren; wait?: Wait | null }) {
	const rows = gateRows(gate, wait?.try_n);
	const hand = rows.findIndex((r) => IN_HAND.has(r.state));
	const end = outcomeRow(gate, docs);
	const cardAtEnd = hand === -1 && !!end?.hand;
	return (
		<section class="t-story" aria-label="The gates">
			{card && hand === -1 && !cardAtEnd && card}
			<ol class="t-sg-list">
				{rows.map((r, i) => {
					const doc = docs?.gates?.find((g) => g.name === r.name);
					const open = i === hand;
					const rule = doc ? sentence(ruleSentence(doc)) : open && wait?.gate === r.name ? waitRule(wait) : '';
					const body =
						rule || (open && card) ? (
							<>
								{rule && <p class="t-sg-rule">{rule}</p>}
								{open && card}
							</>
						) : null;
					return <Row look={r.state} hand={open} open={open} key={r.name} body={body} summary={<><span class="t-sg-n">{i + 1}</span><b class="t-sg-name">{r.name}</b><span class="t-sg-state">{r.words}</span></>} />;
				})}
				{end && (
					<Row
						end
						look={end.look}
						hand={end.hand}
						open={end.open}
						summary={end.line}
						body={
							end.says || cardAtEnd ? (
								<>
									{end.says && <p class="t-sg-rule">{end.says}</p>}
									{cardAtEnd && card}
								</>
							) : null
						}
					/>
				)}
			</ol>
		</section>
	);
}

/** The prompt the task was started with, as written, behind a click. */
export function PromptFold({ prompt }: { prompt: string | null | undefined }) {
	if (!prompt?.trim()) return null;
	return (
		<details class="t-prompt">
			<summary>Show the whole prompt</summary>
			<pre>{prompt}</pre>
		</details>
	);
}
