import { useEffect, useState } from 'preact/hooks';

import { type Entry, SignedOut, type BoxDetail, boxGet, boxList, type BoxRow, type Progress } from './api';

/** The words and the glyph for each state; the controller derives the state, the page only names it. */
const STATES: Record<Progress, { word: string; glyph: string; tone: string }> = {
	needs_you: { word: 'needs you', glyph: '●', tone: 'needs' },
	working: { word: 'working', glyph: '◐', tone: 'working' },
	setting_up: { word: 'setting up', glyph: '◌', tone: 'working' },
	queued: { word: 'queued', glyph: '○', tone: 'quiet' },
	done: { word: 'done', glyph: '✓', tone: 'done' },
	asleep: { word: 'asleep', glyph: '◒', tone: 'quiet' },
	failed: { word: 'failed', glyph: '✕', tone: 'failed' },
	closed: { word: 'closed', glyph: '–', tone: 'quiet' },
};

/** How often an open page asks again. */
const POLL_MS = 4000;

/** A done Box idle this long moves to the Settled shelf. */
const SETTLE_MS = 7 * 24 * 3600 * 1000;

/** Which list a Box sits in. */
type Group = 'needs' | 'working' | 'done' | 'settled';

function groupOf(t: BoxRow, now: number): Group {
	switch (t.state) {
		case 'needs_you':
			return 'needs';
		case 'working':
		case 'setting_up':
		case 'queued':
			return 'working';
		case 'closed':
			return 'settled';
		default:
			return t.state === 'done' && now - Date.parse(t.last_event_at ?? t.updated_at) > SETTLE_MS ? 'settled' : 'done';
	}
}

/** The title a Box goes by: the first line of what it was asked. */
function titleOf(t: { prompt?: string | null; box: string }): string {
	const line = (t.prompt ?? '').split('\n').find((l) => l.trim()) ?? t.box;
	return line.length > 64 ? `${line.slice(0, 63)}…` : line;
}

function age(iso: string | null | undefined, now: number): string {
	const at = iso ? Date.parse(iso) : NaN;
	if (Number.isNaN(at)) return '';
	const s = Math.max(0, Math.round((now - at) / 1000));
	if (s < 60) return 'now';
	if (s < 3600) return `${Math.floor(s / 60)}m`;
	if (s < 86400) return `${Math.floor(s / 3600)}h`;
	return `${Math.floor(s / 86400)}d`;
}

/** Loads on mount and every few seconds while the page is open; keeps the last answer on a failed poll. */
function usePoll<T>(load: () => Promise<T>, key: string) {
	const [value, setValue] = useState<T | null>(null);
	const [error, setError] = useState('');
	const [now, setNow] = useState(Date.now());
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
				if (live) setError(e instanceof SignedOut ? 'Your session ended. Reload to sign in again.' : String((e as Error).message));
			}
		};
		setValue(null);
		void run();
		const timer = setInterval(() => {
			if (!document.hidden) void run();
		}, POLL_MS);
		return () => {
			live = false;
			clearInterval(timer);
		};
	}, [key]);
	return { value, error, now };
}

function Glyph({ state, stale }: { state: Progress; stale: boolean }) {
	const s = STATES[state];
	return (
		<span class={`t-glyph t-${s.tone}${stale ? ' t-stale' : ''}`} role="img" aria-label={s.word} title={s.word}>
			{s.glyph}
		</span>
	);
}

function Row({ t, now, quiet }: { t: BoxRow; now: number; quiet?: boolean }) {
	const s = STATES[t.state];
	return (
		<a class={`t-row t-${s.tone}${quiet ? ' t-settled-row' : ''}`} href={`#/box/${encodeURIComponent(t.box)}`} data-row>
			<Glyph state={t.state} stale={t.stale} />
			<span class="t-repo t-mono">{t.repo ?? ''}</span>
			<span class="t-title">{titleOf(t)}</span>
			<span class={`t-now${t.state === 'needs_you' ? ' t-ask' : ''}`}>
				{t.stale && <span class="t-warn">⚠ stale </span>}
				{t.state === 'failed' && <span class="t-failword">failed </span>}
				{t.line}
			</span>
			<span class="t-age">{age(t.last_event_at ?? t.updated_at, now)}</span>
		</a>
	);
}

function Section({ label, tone, rows, now, empty }: { label: string; tone: string; rows: BoxRow[]; now: number; empty: string }) {
	return (
		<section class={`t-grp t-grp-${tone}`}>
			<h2>
				{label} · {rows.length}
			</h2>
			{rows.length ? rows.map((t) => <Row t={t} now={now} key={t.box} />) : <p class="t-empty">{empty}</p>}
		</section>
	);
}

/** Moves focus between rows with j and k, as the keyboard hint says. */
function useRowKeys() {
	useEffect(() => {
		const on = (e: KeyboardEvent) => {
			const tag = (document.activeElement as HTMLElement | null)?.tagName;
			if (e.metaKey || e.ctrlKey || e.altKey || tag === 'INPUT' || tag === 'TEXTAREA') return;
			if (e.key !== 'j' && e.key !== 'k') return;
			const rows = [...document.querySelectorAll<HTMLElement>('[data-row]')];
			if (!rows.length) return;
			const at = rows.indexOf(document.activeElement as HTMLElement);
			rows[e.key === 'j' ? Math.min(rows.length - 1, at + 1) : Math.max(0, at - 1)]?.focus();
		};
		addEventListener('keydown', on);
		return () => removeEventListener('keydown', on);
	}, []);
}

export function Home() {
	const { value, error, now } = usePoll(boxList, 'home');
	useRowKeys();
	if (!value) {
		return error ? <p class="notice danger" role="alert">{error}</p> : <p class="muted" aria-busy="true">Loading Boxes…</p>;
	}
	const by = (g: Group) => value.filter((t) => groupOf(t, now) === g);
	const settled = by('settled');
	return (
		<div class="t-home">
			{error && <p class="notice danger" role="alert">{error}</p>}
			<p class="t-hint muted">
				Your Boxes · <kbd>j</kbd> <kbd>k</kbd> move · <kbd>enter</kbd> open
			</p>
			<Section label="Needs you" tone="needs" rows={by('needs')} now={now} empty="Nothing needs you." />
			<Section label="Working" tone="working" rows={by('working')} now={now} empty="Nothing is running." />
			<Section label="Done" tone="done" rows={by('done')} now={now} empty="None." />
			<details class="t-settled">
				<summary>
					<span class="t-chev">▸</span> Settled · {settled.length}{' '}
					<span class="muted">Boxes closed, or done and quiet for 7 days</span>
				</summary>
				{settled.map((t) => <Row t={t} now={now} key={t.box} quiet />)}
			</details>
			{!value.length && <p class="t-empty">No Boxes yet. Start one from Claude Code with the moor tools.</p>}
		</div>
	);
}

type Block =
	| { kind: 'fold'; entries: Entry[] }
	| { kind: 'entry'; entry: Entry };

/** Routine steps run together fold into one line; everything else, errors above all, stays. */
function blocks(entries: Entry[]): Block[] {
	const out: Block[] = [];
	for (const entry of entries) {
		const last = out[out.length - 1];
		if (entry.class === 'step' || entry.class === 'routine') {
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

function Line({ e }: { e: Entry }) {
	switch (e.class) {
		case 'error':
			return (
				<div class="t-err" role="alert">
					<b>Something went wrong</b> <span class="t-when">{clock(e.at)}</span>
					<pre>{e.text}</pre>
				</div>
			);
		case 'turn':
			return <div class="t-receipt">{e.text}</div>;
		case 'you':
			return (
				<div class="t-ev t-you">
					<span class="t-who">You</span>
					<span class="t-when">{clock(e.at)}</span>
					<span class="t-what">{e.text}</span>
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
					<span class="t-what t-say">{e.text}</span>
				</div>
			);
	}
}

function Timeline({ detail }: { detail: BoxDetail }) {
	if (!detail.timeline.length) return <p class="t-empty">Nothing has happened yet.</p>;
	return (
		<div class="t-events">
			{blocks(detail.timeline).map((b) =>
				b.kind === 'fold' ? <Fold entries={b.entries} key={b.entries[0]?.id} /> : <Line e={b.entry} key={b.entry.id} />,
			)}
		</div>
	);
}

function EvidenceTab({ detail }: { detail: BoxDetail }) {
	if (!detail.artifacts.length) {
		return <p class="t-empty">No evidence yet. Screenshots, videos, logs and step reports land here.</p>;
	}
	return (
		<div class="t-gallery">
			{detail.artifacts.map((a) => (
				<figure key={`${a.name}@${a.version}`}>
					{a.image ? (
						<a href={a.href} target="_blank" rel="noopener">
							<img src={a.href} alt={a.name} loading="lazy" />
						</a>
					) : (
						<a class="t-file" href={a.href}>
							Download
						</a>
					)}
					<figcaption>
						<b>{a.name}</b>
						<span class="muted">
							{a.kind} · v{a.version} · {Math.max(1, Math.round(a.size / 1024))} KB
						</span>
					</figcaption>
				</figure>
			))}
		</div>
	);
}

const TABS = ['timeline', 'terminal', 'changes', 'evidence'] as const;
export type Tab = (typeof TABS)[number];

/** Tabs that arrive in later steps are shown, disabled, so the page already has its shape. */
const LATER: Partial<Record<Tab, string>> = {
	terminal: 'Terminal comes in a later step',
	changes: 'Changes come in a later step',
};

export function tabOf(raw: string | undefined): Tab {
	return TABS.find((t) => t === raw && !LATER[t]) ?? 'timeline';
}

export function BoxPage({ box, tab }: { box: string; tab: Tab }) {
	const { value, error, now } = usePoll(() => boxGet(box), `box:${box}`);
	if (!value) {
		return (
			<div class="t-box">
				<p><a href="#/">← Boxes</a></p>
				{error ? <p class="notice danger" role="alert">{error}</p> : <p class="muted" aria-busy="true">Loading Box…</p>}
			</div>
		);
	}
	const s = STATES[value.state];
	const base = `#/box/${encodeURIComponent(box)}`;
	const label: Record<Tab, string> = {
		timeline: 'Timeline',
		terminal: 'Terminal',
		changes: 'Changes',
		evidence: `Evidence ${value.artifacts.length}`,
	};
	return (
		<div class="t-box">
			<div class="t-thead">
				<div class="t-crumbs">
					<a class="t-mono" href="#/">{value.repo ?? 'boxes'}</a>
					<span class="t-sep">▸</span>
					<b>{titleOf(value)}</b>
					<span class="t-sep">▸</span>
					<span class="t-mono muted">{value.branch ?? value.box}</span>
					<span class="grow" />
					<span class={`t-pill t-${s.tone}${value.stale ? ' t-stale' : ''}`}>
						{s.glyph} {s.word}
						{value.stale && ' · stale'}
					</span>
				</div>
				<p class="t-prline">
					{value.pr && value.pr.url.startsWith('https://github.com/') ? (
						<>
							<a class="t-mono" href={value.pr.url} rel="noopener">PR #{value.pr.number}</a>
							<span class="muted"> · the agent named it</span>
						</>
					) : (
						<span class="muted">PR: none yet</span>
					)}
				</p>
			</div>
			{error && <p class="notice danger" role="alert">{error}</p>}
			<nav class="t-tabs" role="tablist">
				{TABS.map((t) =>
					LATER[t] ? (
						<span class="t-tab t-disabled" role="tab" aria-disabled="true" title={LATER[t]} key={t}>{label[t]}</span>
					) : (
						<a class="t-tab" role="tab" aria-selected={tab === t} href={t === 'timeline' ? base : `${base}/${t}`} key={t}>{label[t]}</a>
					),
				)}
			</nav>
			<div class="t-body">
				<section>
					{value.state === 'needs_you' && (
						<div class="t-wait" role="region" aria-label="Your input is needed">
							<div class="t-wait-h">Your input is needed</div>
							<p class="t-wait-q">{value.line}</p>
							<p class="muted">Answering from the web comes in a later step. Answer from Claude Code with send_to_box for now.</p>
						</div>
					)}
					{value.stale && (
						<p class="t-stalebar">No event for over 30 minutes. {value.line}</p>
					)}
					{tab === 'evidence' ? <EvidenceTab detail={value} /> : <Timeline detail={value} />}
				</section>
				<aside class="t-rail">
					<h3>Now</h3>
					<p class="t-railline">{value.line}</p>
					<p class="muted">Last event {age(value.last_event_at ?? value.updated_at, now)}</p>
				</aside>
			</div>
		</div>
	);
}
