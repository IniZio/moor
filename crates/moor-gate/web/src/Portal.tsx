import { useEffect, useRef, useState } from 'preact/hooks';

import { type BoxDetail, boxList, type Entry, failure, toError } from './api';
import { TurnLine } from './Changes';
import { whoseHome } from './hashQuery';
import { HomeListView, useMenuDismiss } from './HomeList';
import { Markdown } from './Markdown';
import { MoorLine, routine, ToolFold, ToolLine } from './Transcript';

/** How often an open page asks again. */
export const POLL_MS = 4000;


/** Loads on mount and every few seconds while the page is open; keeps the last answer on a failed poll. */
export function usePoll<T>(load: () => Promise<T>, key: string, every: (value: T | null, now: number) => number = () => POLL_MS) {
	const [value, setValue] = useState<T | null>(null);
	const [error, setError] = useState('');
	const [now, setNow] = useState(Date.now());
	const again = useRef<() => void>(() => undefined);
	useEffect(() => {
		let live = true;
		let last: T | null = null;
		let timer = 0;
		const run = async () => {
			try {
				const next = await load();
				if (live) {
					last = next;
					setValue(next);
					setError('');
					setNow(Date.now());
				}
			} catch (e) {
				if (live) setError(failure(toError(e)));
			}
		};
		const wait = () => {
			timer = window.setTimeout(async () => {
				if (!document.hidden) await run();
				if (live) wait();
			}, every(last, Date.now()));
		};
		setValue(null);
		again.current = () => void run();
		void run().then(() => live && wait());
		return () => {
			live = false;
			clearTimeout(timer);
		};
	}, [key]);
	return { value, error, now, reload: () => again.current() };
}

export type Boxes = ReturnType<typeof useBoxes>;

/** The person's Boxes, one poll the whole page shares: the list, the switcher and the Inbox count. */
export function useBoxes() {
	return usePoll(boxList, 'home');
}

/** The Tasks home: the person's own tasks from the page's shared poll, or everyone's from a poll of its own, so others' tasks never reach the switcher or the Inbox. */
export function Home({ boxes, hash }: { boxes: Boxes; hash: string }) {
	return whoseHome(hash) === 'everyone' ? <EveryonesHome hash={hash} /> : <HomeRows boxes={boxes} hash={hash} />;
}

function EveryonesHome({ hash }: { hash: string }) {
	return <HomeRows boxes={usePoll(() => boxList(true), 'everyone')} hash={hash} />;
}

function HomeRows({ boxes, hash }: { boxes: Boxes; hash: string }) {
	const { value, error, now } = boxes;
	useMenuDismiss();
	if (!value) {
		return error ? <p class="notice danger" role="alert">{error}</p> : <p class="muted" aria-busy="true">Loading tasks…</p>;
	}
	return (
		<>
			{error && <p class="notice danger" role="alert">{error}</p>}
			<HomeListView rows={value} now={now} hash={hash} />
		</>
	);
}

type Block =
	| { kind: 'fold'; entries: Entry[] }
	| { kind: 'tools'; entries: Entry[] }
	| { kind: 'entry'; entry: Entry };

/** Routine steps run together fold into one line; everything else, errors above all, stays. */
function blocks(entries: Entry[]): Block[] {
	const out: Block[] = [];
	for (const entry of entries) {
		const last = out[out.length - 1];
		if (routine(entry)) {
			if (last?.kind === 'tools') last.entries.push(entry);
			else out.push({ kind: 'tools', entries: [entry] });
		} else if (entry.tool) {
			out.push({ kind: 'entry', entry });
		} else if (entry.class === 'step' || entry.class === 'routine') {
			if (last?.kind === 'fold') last.entries.push(entry);
			else out.push({ kind: 'fold', entries: [entry] });
		} else {
			out.push({ kind: 'entry', entry });
		}
	}
	return out;
}

function clock(iso: string): string {
	const d = new Date(iso);
	return Number.isNaN(d.getTime()) ? '' : d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false });
}

/** The lines that only setting a Box up writes. */
const SETUP = new Set(['setting_up', 'box_requested', 'box_ready', 'cloned']);

/** Whether a fold of lines is the Box being set up: routine lines only, with one that only setup writes. */
const isSetup = (entries: Entry[]) => entries.every((e) => e.class === 'routine') && entries.some((e) => SETUP.has(e.kind) || e.kind.startsWith('setup'));

/** Folded routine lines; the fold that sets the Box up reads as setting it up on `machine`. */
function Fold({ entries, machine }: { entries: Entry[]; machine: string | null | undefined }) {
	const steps = entries.filter((e) => e.class === 'step').length;
	const notes = entries.length - steps;
	const said =
		machine && isSetup(entries)
			? `Set up the Box on ${machine}`
			: [steps && `${steps} tool call${steps === 1 ? '' : 's'}`, notes && `${notes} setup note${notes === 1 ? '' : 's'}`].filter(Boolean).join(', ');
	return (
		<details class="t-fold">
			<summary>
				<span class="t-chev">▸</span> {said}
			</summary>
			<ul class="t-fold-body">
				{entries.map((e) => (
					<li key={e.id}>
						<span class="t-when">{clock(e.at)}</span> <span class={e.class === 'step' ? 't-mono' : 'muted'}>{e.text}</span>
					</li>
				))}
			</ul>
		</details>
	);
}

/** One timeline line; `you` names who wrote to the agent, the reader unless the task is another person's. */
export function Line({ e, gates, you = 'You' }: { e: Entry; gates: readonly string[]; you?: string }) {
	switch (e.class) {
		case 'moor':
			return <MoorLine e={e} when={clock(e.at)} gates={gates} />;
		case 'error':
			return (
				<div class="t-err" role="alert">
					<b>Something went wrong</b> <span class="t-when">{clock(e.at)}</span>
					<pre>{e.text}</pre>
				</div>
			);
		case 'you':
			return (
				<div class="t-ev t-you">
					<span class="t-who">{you}</span>
					<span class="t-when">{clock(e.at)}</span>
					<span class="t-what">{e.text === '(interrupt)' ? <em>Interrupted the agent's turn</em> : e.text}</span>
					{e.step && <span class="t-step">Step: {e.step}</span>}
					{e.kind === 'typed' && <span class="t-step">typed in the terminal</span>}
				</div>
			);
		case 'asked':
			return (
				<div class="t-ev">
					<span class="t-who t-ask-mark">●</span>
					<span class="t-when">{clock(e.at)}</span>
					<span class="t-what">Asked: {e.text}</span>
				</div>
			);
		case 'evidence':
			return (
				<div class="t-ev">
					<span class="t-who">Agent</span>
					<span class="t-when">{clock(e.at)}</span>
					<span class="t-what">
						Published <span class="t-mono">{e.text}</span>
					</span>
				</div>
			);
		default:
			return (
				<div class="t-ev">
					<span class="t-who">Agent</span>
					<span class="t-when">{clock(e.at)}</span>
					<div class="t-what t-agent-said">
						<Markdown text={e.text} />
					</div>
				</div>
			);
	}
}

export function Timeline({ detail }: { detail: BoxDetail }) {
	if (!detail.timeline.length) return <p class="t-empty">Nothing has happened yet.</p>;
	return (
		<div class="t-events">
			{blocks(detail.timeline).map((b) =>
				b.kind === 'fold' ? (
					<Fold entries={b.entries} machine={detail.machine} key={b.entries[0]?.id} />
				) : b.kind === 'tools' ? (
					<ToolFold entries={b.entries} when={clock} key={b.entries[0]?.key ?? b.entries[0]?.id} />
				) : b.entry.tool ? (
					<ToolLine e={b.entry} when={clock(b.entry.at)} key={b.entry.key ?? b.entry.id} />
				) : b.entry.class === 'turn' ? (
					<TurnLine e={b.entry} entries={detail.timeline} href={`#/box/${encodeURIComponent(detail.box)}/changes`} key={b.entry.id} />
				) : (
					<Line e={b.entry} gates={detail.gate?.gates ?? []} you={detail.owner ?? 'You'} key={b.entry.key ?? b.entry.id} />
				),
			)}
		</div>
	);
}
