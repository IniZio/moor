import type { ComponentChildren } from 'preact';

import type { BoxDetail, Entry, Evidence } from './api';
import { Markdown } from './Markdown';
import { ArtifactCard, BoxLine, OutputCard, PrCard } from './PlaceCards';
import { exactTime } from './HomeList';
import { clock, Line } from './Portal';
import { foldWords, routine, ToolLine } from './Transcript';

/** Something the agent said, under it the calls and notes that led up to it; or, before it speaks again, the calls since it last did. */
type Beat = { line?: Entry; work: Entry[] };

/** One message of the person and what the agent did after it, and the failure that ended the task when it ended there. */
type Turn = { you?: Entry; beats: Beat[]; ended?: Entry };

const failing = (e: Entry) => e.class === 'error' || !!e.tool?.failed;

/** The question's task cut at each message of the person and, inside a turn, at each message of the agent; setup notes are dropped. When the task failed, the last failure nothing from the agent followed ends it. */
function turnsOf(entries: Entry[], failed: boolean): Turn[] {
	const turns: Turn[] = [];
	let turn: Turn = { beats: [] };
	let work: Entry[] = [];
	const close = (ends = false) => {
		const at = ends ? lastFailing(work) : -1;
		if (at >= 0) turn.ended = work.splice(at, 1)[0];
		if (work.length) turn.beats.push({ work });
		if (turn.you || turn.beats.length || turn.ended) turns.push(turn);
		turn = { beats: [] };
		work = [];
	};
	for (const e of entries) {
		if (e.class === 'you') {
			close();
			turn.you = e;
		} else if (e.class === 'message') {
			turn.beats.push({ line: e, work });
			work = [];
		} else if (e.class !== 'routine' && e.class !== 'hidden' && e.class !== 'turn') {
			work.push(e);
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

/** A part of a turn's conversation: something the agent said with its fold, or what its work Boxes made, in the order they happened. */
type Part = { at: number; nodes: ComponentChildren[]; rank: number; said?: boolean };

/** At the same moment the pull request card comes first, then the agent's words, then the rest. */
const PR = 0;
const SAID = 1;
const MADE = 2;
const byTime = (x: Part, y: Part) => x.at - y.at || x.rank - y.rank;

const fold = (work: Entry[], gates: readonly string[], key: string) => (
	<details class="t-pl-fold" key={key}>
		<summary>
			<span class="t-chev">▸</span> {workWords(work)}
		</summary>
		<div class="t-pl-body">
			{work.map((e) => (
				<Work entry={e} gates={gates} key={e.key ?? e.id} />
			))}
		</div>
	</details>
);

const beatPart = ({ line, work }: Beat, gates: readonly string[]): Part => {
	const first = line ?? (work[0] as Entry);
	const id = first.key ?? first.id;
	const folded = work.length ? [fold(work, gates, `f${id}`)] : [];
	if (!line) return { at: Date.parse(first.at), rank: SAID, nodes: folded };
	return {
		at: Date.parse(line.at),
		rank: SAID,
		said: true,
		nodes: [
			<div class="t-pl-say" key={`s${id}`}>
				<span class="t-pl-mark">◆</span>
				<div class="t-pl-text">
					<Markdown text={line.text} />
				</div>
			</div>,
			...folded,
		],
	};
};

/** When a turn began: its message, or the first thing it holds when the agent spoke first. */
const startOf = (t: Turn): number => Math.min(...[t.you, ...t.beats.flatMap((b) => [b.line, ...b.work])].flatMap((e) => (e ? [Date.parse(e.at)] : [])));

/** The last index of `items` that `holds`, or -1. */
const lastIndex = <T,>(items: readonly T[], holds: (item: T) => boolean): number => items.reduce((at, item, i) => (holds(item) ? i : at), -1);

/** Whether the pull request is the one a workflow run was started for, which its head names, rather than one it made. */
const ranFor = (d: BoxDetail) => !!d.pr && !!d.workflow?.pull?.endsWith(`#${d.pr.number}`);

/** When the pull request first showed in the task: the first entry whose words or call mention its address, or NaN when none does. */
function prSeenAt(pr: NonNullable<BoxDetail['pr']>, timeline: Entry[]): number {
	const first = timeline.find((e) => [e.text, e.tool?.output?.text, JSON.stringify(e.tool?.input ?? '')].some((s) => s?.includes(pr.url)));
	return first ? Date.parse(first.at) : NaN;
}

/** The parts of each turn: what the agent said, and the work Box lines and cards that happened in it. A Box part goes to the last turn that began no later than it, or the first. The pull request card sits where the pull request first showed, after the Box line of the Box that opened it. When nothing shows it, the card sits above the agent's last words in the last turn, or at its end when there are none. */
function partsOf(detail: BoxDetail, turns: Turn[], prState: ComponentChildren): Part[][] {
	const gates = detail.gate?.gates ?? [];
	const starts = turns.map(startOf);
	const parts: Part[][] = turns.map((t) => t.beats.map((b) => beatPart(b, gates)));
	const put = (at: number, node: ComponentChildren, rank = MADE) => parts[Math.max(0, lastIndex(starts, (s) => s <= at))]?.push({ at, nodes: [node], rank });
	const cards = (from: string, files: Evidence[]) => {
		const newest = (name: string) => Math.max(...files.flatMap((f) => (f.name === name ? [f.version] : [])));
		for (const a of files) put(Date.parse(a.created_at), <ArtifactCard a={a} newest={newest(a.name)} key={`a${from}/${a.name}@${a.version}`} />);
	};
	for (const box of detail.task_boxes) {
		put(Date.parse(box.started_at), <BoxLine box={box} key={`b${box.id}`} />);
		cards(String(box.id), box.artifacts);
	}
	if (!detail.task_at) cards('own', detail.artifacts);
	const pr = ranFor(detail) ? null : detail.pr;
	for (const o of detail.outputs) if (o.url !== pr?.url) put(o.first_at, <OutputCard o={o} key={`o${o.url}`} />);
	const seen = pr ? prSeenAt(pr, detail.timeline) : NaN;
	if (pr && !Number.isNaN(seen)) put(seen, <PrCard pr={pr} state={prState} key="pr" />, PR);
	for (const list of parts) list.sort(byTime);
	const last = parts.at(-1);
	if (pr && Number.isNaN(seen) && last) {
		const above = lastIndex(last, (p) => !!p.said);
		last.splice(above < 0 ? last.length : above, 0, { at: 0, rank: PR, nodes: [<PrCard pr={pr} state={prState} key="pr" />] });
	}
	return parts;
}

function Work({ entry, gates }: { entry: Entry; gates: readonly string[] }) {
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

/** A workflow run's first message is its automation's prompt: moor sent it, no person wrote it. */
const runPrompt = (d: BoxDetail): Entry | undefined => (d.workflow ? d.timeline.find((e) => e.class === 'you') : undefined);

/** The line a run opens with in place of the prompt, which `show` unfolds. */
const firstLine = (prompt: string) => (
	<details class="t-pl-first" key="first">
		<summary>
			Started by the automation, with its prompt. <span class="t-qlnk">show</span>
		</summary>
		<pre class="t-pl-prompt">{prompt}</pre>
	</details>
);

/** When the person wrote: the clock, with the day too once it is not the day the task began. */
const stamp = (at: string, began: string) => exactTime(at, Date.parse(began));

/** A question's task, or a workflow run opened by its automation's line, as a conversation: the person's messages as bubbles under their name and time, then in the order they happened everything the agent said as a line with a quiet fold of the calls that led to it, the work Boxes it started as one line each, and what they made as cards. Only a failure that ended the task shows as an error. `end` closes the conversation, as the agent's open question does. `prState` says where the pull request stands, once the page has read it. */
export function PlaceStory({ detail, you, prState, end }: { detail: BoxDetail; you: string; prState?: ComponentChildren; end?: ComponentChildren }) {
	const prompt = runPrompt(detail);
	const first = prompt ? [firstLine(prompt.text)] : [];
	const turns = turnsOf(
		detail.timeline.filter((e) => e !== prompt),
		detail.state === 'failed',
	);
	if (!turns.length) return end || prompt ? <div class="t-pl">{[...first, end]}</div> : <p class="t-empty">Nothing has happened yet.</p>;
	const parts = partsOf(detail, turns, prState);
	const began = prompt?.at ?? turns.find((t) => t.you)?.you?.at ?? '';
	return (
		<div class="t-pl">
			{[
				...first,
				...turns.flatMap((t, i) => [
				...(t.you
					? [
							<div class="t-pl-you" key={`y${i}`}>
								<span class="t-pl-meta">
									{you} · {stamp(t.you.at, began)}
								</span>
								<span class="t-pl-bubble">{t.you.text === '(interrupt)' ? <em>Interrupted the agent's turn</em> : t.you.text}</span>
							</div>,
						]
					: []),
				...(parts[i] ?? []).flatMap((p) => p.nodes),
				...(t.ended ? [<Ended e={t.ended} key={`e${i}`} />] : []),
				]),
				...(end ? [end] : []),
			]}
		</div>
	);
}
