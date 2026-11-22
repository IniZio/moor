import type { ComponentChildren } from 'preact';

import type { Workflow } from './api';
import { closingRow, GATE_LOOK, type GateRow, type GateView, gateRows, ruleSentence } from './gate';
import './story.css';

/** The states in which a gate is in hand. */
const IN_HAND = new Set<GateRow['state']>(['working', 'asking', 'stuck', 'failed']);

type Docs = Pick<Workflow, 'gates' | 'outcomes'>;

/** A gate's rule starts a sentence, except where it starts with the name moor. */
const sentence = (rule: string) => (rule.startsWith('moor') ? rule : rule.charAt(0).toUpperCase() + rule.slice(1));

function Row({ look, hand, open, body, summary }: { look: GateRow['state']; hand: boolean; open: boolean; body: ComponentChildren; summary: ComponentChildren }) {
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
		<li class={`t-sg t-${tone}${hand ? ' t-sg-hand' : ''}`}>
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
		return { look: o.tone === 'good' ? 'passed' : o.tone, hand: false, open: true, line: <>ended as: <b>{o.name}</b></>, says: o.says };
	}
	const closing = closingRow(gate);
	if (closing) return { look: closing.state, hand: IN_HAND.has(closing.state), open: IN_HAND.has(closing.state), line: <><span class="t-sg-name">outcome</span><span class="t-sg-state">{closing.words}</span></> };
	const names = (docs?.outcomes ?? []).map((o) => o.name);
	if (names.length === 0 || gate.run_ended) return null;
	return { look: 'later', hand: false, open: false, line: <>ends as one of: {names.join(', ')}</> };
}

/** One row per gate with its mark, number, name and state; the gate in hand is open, with what must be true and the card in place. Ends with the outcome. */
export function GateStory({ gate, docs, card }: { gate: GateView; docs: Docs | null; card: ComponentChildren }) {
	const rows = gateRows(gate);
	const hand = rows.findIndex((r) => IN_HAND.has(r.state));
	const end = outcomeRow(gate, docs);
	const cardAtEnd = hand === -1 && !!end?.hand;
	return (
		<section class="t-story" aria-label="The gates">
			{card && hand === -1 && !cardAtEnd && card}
			<ol class="t-sg-list">
				{rows.map((r, i) => {
					const doc = docs?.gates?.find((g) => g.name === r.name);
					const rule = doc ? ruleSentence(doc) : '';
					const open = i === hand;
					const body =
						rule || (open && card) ? (
							<>
								{rule && <p class="t-sg-rule">{sentence(rule)}</p>}
								{open && card}
							</>
						) : null;
					return <Row look={r.state} hand={open} open={open} key={r.name} body={body} summary={<><span class="t-sg-n">{i + 1}</span><b class="t-sg-name">{r.name}</b><span class="t-sg-state">{r.words}</span></>} />;
				})}
				{end && (
					<Row
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
			<summary>The prompt</summary>
			<pre>{prompt}</pre>
		</details>
	);
}
