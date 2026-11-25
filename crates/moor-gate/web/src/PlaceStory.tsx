import type { ComponentChildren } from 'preact';

import type { BoxDetail, Entry } from './api';
import { Markdown } from './Markdown';
import { ArtifactCard, BoxLine, PrCard } from './PlaceCards';
import { clock, Line } from './Portal';
import { foldWords, routine, ToolLine } from './Transcript';

/** One question and what came of it: the person's message, the agent's last words as the answer, and everything else the agent did or said before them as its work. */
type Turn = { you?: Entry; answer?: Entry; work: Entry[]; ended?: Entry };

const failing = (e: Entry) => e.class === 'error' || !!e.tool?.failed;

/** The question's task cut at each message of the person; a turn's setup notes are dropped, its last words are the answer, and its calls, errors and earlier words are its work. When the task failed, the last failure nothing from the agent followed ends it. */
function turnsOf(entries: Entry[], failed: boolean): Turn[] {
	const turns: Turn[] = [];
	let turn: Turn = { work: [] };
	/** Takes the turn's last words out of its work, and returns how many work entries came before them. */
	const settle = (): number => {
		const at = turn.work.map((e) => e.class).lastIndexOf('message');
		if (at >= 0) turn.answer = turn.work.splice(at, 1)[0];
		return Math.max(at, 0);
	};
	const close = (ends = false) => {
		const settled = settle();
		if (ends) {
			const at = lastFailing(turn.work);
			if (at >= settled) turn.ended = turn.work.splice(at, 1)[0];
		}
		if (turn.you || turn.answer || turn.work.length || turn.ended) turns.push(turn);
		turn = { work: [] };
	};
	for (const e of entries) {
		if (e.class === 'you') {
			close();
			turn.you = e;
		} else if (e.class !== 'routine' && e.class !== 'hidden' && e.class !== 'turn') {
			turn.work.push(e);
		}
	}
	close(failed);
	return turns;
}

const lastFailing = (work: Entry[]) => work.map(failing).lastIndexOf(true);

const plural = (n: number, one: string, more: string) => `${n} ${n === 1 ? one : more}`;

/** What the fold of a turn's work says: the reads and searches the transcript names, then every other call as a command. */
function workWords(work: Entry[]): string {
	const calls = work.filter((e) => e.tool || e.class === 'step');
	const grouped = calls.filter(routine);
	const ran = calls.length - grouped.length;
	const said = [grouped.length ? foldWords(grouped) : '', ran ? `${grouped.length ? 'ran' : 'Ran'} ${plural(ran, 'command', 'commands')}` : ''].filter(Boolean).join(', ');
	return said || plural(work.length, 'note', 'notes');
}

/** A part of a turn's conversation: the agent's answer, or what its work Boxes made, in the order they happened. */
type Part = { at: number; node: ComponentChildren; answer?: boolean; pr?: boolean };

/** Parts in time order; at the same moment the pull request card comes first, then the answer, then the rest. */
const byTime = (x: Part, y: Part) => x.at - y.at || rank(x) - rank(y);
const rank = (p: Part) => (p.pr ? 0 : p.answer ? 1 : 2);

const answerPart = (e: Entry): Part => ({
	at: Date.parse(e.at),
	answer: true,
	node: (
		<div class="t-pl-say" key={`s${e.key ?? e.id}`}>
			<span class="t-pl-mark">◆</span>
			<div class="t-pl-text">
				<Markdown text={e.text} />
			</div>
		</div>
	),
});

/** When a turn began: its message, or the first thing it holds when the agent spoke first. */
const startOf = (t: Turn): number => Math.min(...[t.you, t.answer, ...t.work].flatMap((e) => (e ? [Date.parse(e.at)] : [])));

/** The last index of `items` that `holds`, or -1. */
const lastIndex = <T,>(items: readonly T[], holds: (item: T) => boolean): number => items.reduce((at, item, i) => (holds(item) ? i : at), -1);

/** When the pull request first showed in the task: the first entry whose words or call mention its address, or NaN when none does. */
function prSeenAt(pr: NonNullable<BoxDetail['pr']>, timeline: Entry[]): number {
	const first = timeline.find((e) => [e.text, e.tool?.output?.text, JSON.stringify(e.tool?.input ?? '')].some((s) => s?.includes(pr.url)));
	return first ? Date.parse(first.at) : NaN;
}

/** The parts of each turn: its answer, and the work Box lines and cards that happened in it. A Box part goes to the last turn that began no later than it, or the first, and follows the answer of the same moment. The pull request card sits where the pull request first showed, after the Box line of the Box that opened it. When nothing shows it, the card sits above the answer of the last turn, or at its end when there is none. */
function partsOf(detail: BoxDetail, turns: Turn[], prState: ComponentChildren): Part[][] {
	const starts = turns.map(startOf);
	const parts: Part[][] = turns.map((t) => (t.answer ? [answerPart(t.answer)] : []));
	const put = (at: number, node: ComponentChildren, pr = false) => parts[Math.max(0, lastIndex(starts, (s) => s <= at))]?.push({ at, node, pr });
	for (const box of detail.task_boxes) {
		put(Date.parse(box.started_at), <BoxLine box={box} key={`b${box.id}`} />);
		for (const a of box.artifacts) put(Date.parse(a.created_at), <ArtifactCard a={a} key={`a${box.id}/${a.name}@${a.version}`} />);
	}
	const { pr } = detail;
	const seen = pr ? prSeenAt(pr, detail.timeline) : NaN;
	if (pr && !Number.isNaN(seen)) put(seen, <PrCard pr={pr} state={prState} key="pr" />, true);
	for (const list of parts) list.sort(byTime);
	const last = parts.at(-1);
	if (pr && Number.isNaN(seen) && last) {
		const above = lastIndex(last, (p) => !!p.answer);
		last.splice(above < 0 ? last.length : above, 0, { at: 0, node: <PrCard pr={pr} state={prState} key="pr" /> });
	}
	return parts;
}

function Work({ entry, gates }: { entry: Entry; gates: readonly string[] }) {
	if (entry.class === 'message')
		return (
			<div class="t-pl-note">
				<Markdown text={entry.text} />
			</div>
		);
	if (entry.tool) return <ToolLine e={entry} when={clock(entry.at)} />;
	if (entry.class === 'step') return <p class="t-pl-step t-mono">{entry.text}</p>;
	return <Line e={entry} gates={gates} />;
}

function Ended({ e }: { e: Entry }) {
	return (
		<div class="t-pl-ended" role="alert">
			<p class="t-pl-ended-h">{e.tool ? e.tool.title : 'Something went wrong'}</p>
			<pre>{e.tool?.output?.text ?? e.text}</pre>
		</div>
	);
}

/** A question's task as a conversation: the person's messages as bubbles under their name and time, the work Boxes its agent started as one line each with what they made as cards, the agent's last words as a line, one quiet fold of its work and earlier words under it, and only a failure that ended the task as an error. `prState` says where the pull request stands, once the page has read it. */
export function PlaceStory({ detail, you, prState }: { detail: BoxDetail; you: string; prState?: ComponentChildren }) {
	const turns = turnsOf(detail.timeline, detail.state === 'failed');
	if (!turns.length) return <p class="t-empty">Nothing has happened yet.</p>;
	const gates = detail.gate?.gates ?? [];
	const parts = partsOf(detail, turns, prState);
	return (
		<div class="t-pl">
			{turns.flatMap((t, i) => [
				...(t.you
					? [
							<div class="t-pl-you" key={`y${i}`}>
								<span class="t-pl-meta">
									{you} · {clock(t.you.at)}
								</span>
								<span class="t-pl-bubble">{t.you.text === '(interrupt)' ? <em>Interrupted the agent's turn</em> : t.you.text}</span>
							</div>,
						]
					: []),
				...(parts[i] ?? []).map((p) => p.node),
				...(t.work.length
					? [
							<details class="t-pl-fold" key={`f${i}`}>
								<summary>
									<span class="t-chev">▸</span> {workWords(t.work)}
								</summary>
								<div class="t-pl-body">
									{t.work.map((e) => (
										<Work entry={e} gates={gates} key={e.key ?? e.id} />
									))}
								</div>
							</details>,
						]
					: []),
				...(t.ended ? [<Ended e={t.ended} key={`e${i}`} />] : []),
			])}
		</div>
	);
}
