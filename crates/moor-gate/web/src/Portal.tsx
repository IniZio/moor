import { useEffect, useRef, useState } from 'preact/hooks';

import { type BoxDetail, boxList, type Entry, failure, toError } from './api';
import { TurnLine } from './Changes';
import { HomeListView, useMenuDismiss } from './HomeList';
import { Markdown } from './Markdown';
import { MoorLine, routine, ToolFold, ToolLine } from './Transcript';

/** How often an open page asks again. */
const POLL_MS = 4000;


export function age(iso: string | null | undefined, now: number): string {
	const at = iso ? Date.parse(iso) : NaN;
	if (Number.isNaN(at)) return '';
	const s = Math.max(0, Math.round((now - at) / 1000));
	if (s < 60) return 'now';
	if (s < 3600) return `${Math.floor(s / 60)}m`;
	if (s < 86400) return `${Math.floor(s / 3600)}h`;
	return `${Math.floor(s / 86400)}d`;
}
/** Loads on mount and every few seconds while the page is open; keeps the last answer on a failed poll. */
export function usePoll<T>(load: () => Promise<T>, key: string) {
	const [value, setValue] = useState<T | null>(null);
	const [error, setError] = useState('');
	const [now, setNow] = useState(Date.now());
	const again = useRef<() => void>(() => undefined);
	useEffect(() => {
		let live = true;
		const run = async () => {
			try {
				const next = await load();
				if (live) {
					setValue(next);
					setError('');
					setNow(Date.now());
				}
			} catch (e) {
				if (live) setError(failure(toError(e)));
			}
		};
		setValue(null);
		again.current = () => void run();
		void run();
		const timer = setInterval(() => {
			if (!document.hidden) void run();
		}, POLL_MS);
		return () => {
			live = false;
			clearInterval(timer);
		};
	}, [key]);
	return { value, error, now, reload: () => again.current() };
}

export type Boxes = ReturnType<typeof useBoxes>;

/** The person's Boxes, one poll the whole page shares: the list, the switcher and the Inbox count. */
export function useBoxes() {
	return usePoll(boxList, 'home');
}

export function Home({ boxes, hash }: { boxes: Boxes; hash: string }) {
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

function Fold({ entries }: { entries: Entry[] }) {
	const steps = entries.filter((e) => e.class === 'step').length;
	const notes = entries.length - steps;
	const said = [steps && `${steps} tool call${steps === 1 ? '' : 's'}`, notes && `${notes} setup note${notes === 1 ? '' : 's'}`].filter(Boolean).join(', ');
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

export function Line({ e, gates }: { e: Entry; gates: readonly string[] }) {
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
					<span class="t-who">You</span>
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
					<Fold entries={b.entries} key={b.entries[0]?.id} />
				) : b.kind === 'tools' ? (
					<ToolFold entries={b.entries} when={clock} key={b.entries[0]?.key ?? b.entries[0]?.id} />
				) : b.entry.tool ? (
					<ToolLine e={b.entry} when={clock(b.entry.at)} key={b.entry.key ?? b.entry.id} />
				) : b.entry.class === 'turn' ? (
					<TurnLine e={b.entry} entries={detail.timeline} href={`#/box/${encodeURIComponent(detail.box)}/changes`} key={b.entry.id} />
				) : (
					<Line e={b.entry} gates={detail.gate?.gates ?? []} key={b.entry.key ?? b.entry.id} />
				),
			)}
		</div>
	);
}
