import type { ComponentChildren } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import * as v from 'valibot';

import {
	type Account,
	type BoxDetail,
	boxAnswer,
	boxGet,
	boxList,
	type BoxRow,
	boxStart,
	type Entry,
	type Needs,
	type Pick,
	type Progress,
	repoList,
	runAnswer,
	SignedOut,
} from './api';
import { ChangesTab, changesLabel, TurnLine } from './Changes';
import { HeaderActions } from './Ending';
import { PlanCard } from './PlanCard';
import { PrHeader } from './PrHeader';
import { Markdown } from './Markdown';
import { RepoPicker } from './RepoPicker';
import { FailedCard, takeAgain } from './Retry';
import { Composer } from './Steer';
import { MakeRecurring } from './WorkflowForm';
import { TerminalTab } from './Terminal';
import { routine, ToolFold, ToolLine } from './Transcript';

/** The words and the glyph for each state; the controller derives the state, the page only names it. */
export const STATES: Record<Progress, { word: string; glyph: string; tone: string }> = {
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
				if (live) setError(e instanceof SignedOut ? 'Your session ended. Reload to sign in again.' : String((e as Error).message));
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

export function Glyph({ state, stale }: { state: Progress; stale: boolean }) {
	const s = STATES[state];
	return (
		<span class={`t-glyph t-${s.tone}${stale ? ' t-stale' : ''}`} role="img" aria-label={s.word} title={s.word}>
			{s.glyph}
		</span>
	);
}

/** The repos cloned beside the first: each one up to two, else a count with the names on hover. */
function OtherRepos({ repos }: { repos: string[] }) {
	if (repos.length === 0) return null;
	if (repos.length > 2) {
		return (
			<span class="t-more t-mono" title={repos.join(', ')}>
				+{repos.length} more
			</span>
		);
	}
	return (
		<>
			{repos.map((r) => (
				<span key={r} class="t-more t-mono" title="Cloned beside the first repo">
					+ {r}
				</span>
			))}
		</>
	);
}

function Row({ t, now, quiet }: { t: BoxRow; now: number; quiet?: boolean }) {
	const s = STATES[t.state];
	return (
		<a class={`t-row t-${s.tone}${quiet ? ' t-settled-row' : ''}`} href={`#/box/${encodeURIComponent(t.box)}`} data-row>
			<Glyph state={t.state} stale={t.stale} />
			<span class="t-repo t-mono">
				<span class="t-repo-name">{t.repo ?? ''}</span>
				{t.other_repos.length > 0 && (
					<span class="t-more" title={t.other_repos.join(', ')}>
						+{t.other_repos.length}
					</span>
				)}
			</span>
			<span class="t-title">{t.title}</span>
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
			if (e.metaKey || e.ctrlKey || e.altKey || tag === 'INPUT' || tag === 'TEXTAREA' || document.querySelector('dialog[open]')) return;
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

export type Boxes = ReturnType<typeof useBoxes>;

/** The person's Boxes, one poll the whole page shares: the list, the switcher and the Inbox count. */
export function useBoxes() {
	return usePoll(boxList, 'home');
}

export function Home({ boxes }: { boxes: Boxes }) {
	const { value, error, now } = boxes;
	useRowKeys();
	if (!value) {
		return error ? <p class="notice danger" role="alert">{error}</p> : <p class="muted" aria-busy="true">Loading tasks…</p>;
	}
	const by = (g: Group) => value.filter((t) => groupOf(t, now) === g);
	const settled = by('settled');
	return (
		<div class="t-home">
			{error && <p class="notice danger" role="alert">{error}</p>}
			<p class="t-hint muted">
				<kbd>j</kbd> <kbd>k</kbd> move · <kbd>enter</kbd> open
			</p>
			<Section label="Needs you" tone="needs" rows={by('needs')} now={now} empty="Nothing needs you." />
			<Section label="Working" tone="working" rows={by('working')} now={now} empty="Nothing is running." />
			<Section label="Done" tone="done" rows={by('done')} now={now} empty="None." />
			<details class="t-settled">
				<summary>
					<span class="t-chev">▸</span> Settled · {settled.length}{' '}
					<span class="muted">tasks closed, or done and quiet for 7 days</span>
				</summary>
				{settled.map((t) => <Row t={t} now={now} key={t.box} quiet />)}
			</details>
			{!value.length && <p class="t-empty">No tasks yet. Start one with New task, or from Claude Code with the moor tools.</p>}
		</div>
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

function Line({ e }: { e: Entry }) {
	switch (e.class) {
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
					<div class="t-what t-say">
						<Markdown text={e.text} />
					</div>
				</div>
			);
	}
}

function Timeline({ detail }: { detail: BoxDetail }) {
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
					<Line e={b.entry} key={b.entry.key ?? b.entry.id} />
				),
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

export function tabOf(raw: string | undefined): Tab {
	return TABS.find((t) => t === raw) ?? 'timeline';
}

/** The buttons a card shows: a permission has three fixed ones, a question has all its options. */
export function buttonsOf(needs: Needs): { pick: Pick; word: string; tone: string }[] {
	if (needs.kind === 'question') {
		return needs.picks.map((pick) => ({ pick, word: pick.label, tone: 'choice' }));
	}
	const first = (role: Pick['role']) => needs.picks.find((p) => p.role === role);
	const out = [
		{ pick: first('deny'), word: 'Deny', tone: 'deny' },
		{ pick: first('allow_once'), word: 'Allow once', tone: 'once' },
		{ pick: first('allow_box'), word: 'Allow for this task', tone: 'box' },
	];
	return out.flatMap((b) => (b.pick ? [{ ...b, pick: b.pick }] : []));
}

/** The card for an agent that asks; the controller wrote what it says, the page only sends the pick. */
function NeedsCard({ box, needs, csrf, onSent }: { box: string; needs: Needs; csrf: string; onSent: () => void }) {
	return needs.kind === 'approve' && needs.approve ? (
		<ApproveCard needs={needs} run={needs.approve} csrf={csrf} onSent={onSent} />
	) : (
		<AskCard box={box} needs={needs} csrf={csrf} onSent={onSent} />
	);
}

/** An agent's own prompt: a permission or a question. */
function AskCard({ box, needs, csrf, onSent }: { box: string; needs: Needs; csrf: string; onSent: () => void }) {
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState('');
	const [typed, setTyped] = useState('');
	const send = async (n: number, text?: string) => {
		if (busy) return;
		setBusy(true);
		setError('');
		try {
			await boxAnswer(csrf, box, needs.id, n, text);
			onSent();
		} catch (e) {
			setError(e instanceof SignedOut ? 'Your session ended. Reload to sign in again.' : String((e as Error).message));
			onSent();
		} finally {
			setBusy(false);
		}
	};
	const permission = needs.kind === 'permission';
	return (
		<div class="t-wait" role="region" aria-label="Your input is needed">
			<div class="t-wait-h">{permission ? 'The agent asks permission' : 'Your input is needed'}</div>
			<p class="t-wait-q">{needs.question}</p>
			{needs.detail && <pre class="t-quote">{needs.detail}</pre>}
			<div class="t-wait-btns">
				{buttonsOf(needs).map(({ pick, word, tone }) => (
					<button type="button" class={`t-btn t-btn-${tone}`} disabled={busy} onClick={() => void send(pick.n)} key={pick.n}>
						{word}
					</button>
				))}
			</div>
			{permission && <p class="muted t-wait-note">Or write to the agent below. A message denies this request and passes the message on.</p>}
			{needs.other != null && (
				<form
					class="t-wait-own"
					onSubmit={(e) => {
						e.preventDefault();
						if (typed.trim() && needs.other != null) void send(needs.other, typed);
					}}
				>
					<input
						value={typed}
						onInput={(e) => setTyped(e.currentTarget.value)}
						placeholder="Or answer in your own words"
						aria-label="Your own answer"
						disabled={busy}
					/>
					<button type="submit" class="t-btn t-btn-once" disabled={busy || !typed.trim()}>
						Send
					</button>
				</form>
			)}
			{error && (
				<p class="notice danger" role="alert">
					{error}
				</p>
			)}
		</div>
	);
}

/** The card for a workflow run that waits at an approve step: its question, yes or no, and a note for the agent. */
function ApproveCard({ needs, run, csrf, onSent }: { needs: Needs; run: { workflow: string; run: number; step: string }; csrf: string; onSent: () => void }) {
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState('');
	const [note, setNote] = useState('');
	const send = async (yes: boolean) => {
		if (busy) return;
		setBusy(true);
		setError('');
		try {
			await runAnswer(csrf, run.workflow, run.run, yes, note);
			onSent();
		} catch (e) {
			setError(e instanceof SignedOut ? 'Your session ended. Reload to sign in again.' : String((e as Error).message));
			onSent();
		} finally {
			setBusy(false);
		}
	};
	return (
		<div class="t-wait" role="region" aria-label="Your approval is needed">
			<div class="t-wait-h">Step {run.step} is waiting for approval</div>
			<p class="t-wait-q">{needs.question}</p>
			<form
				class="t-wait-own"
				onSubmit={(e) => {
					e.preventDefault();
					void send(true);
				}}
			>
				<button type="button" class="t-btn t-btn-deny" disabled={busy} onClick={() => void send(false)}>
					Reject
				</button>
				<button type="submit" class="t-btn" disabled={busy}>
					Approve
				</button>
				<input
					value={note}
					onInput={(e) => setNote(e.currentTarget.value)}
					placeholder="Add a note for the agent (optional)"
					aria-label="Note for the agent"
					disabled={busy}
				/>
			</form>
			{error && (
				<p class="notice danger" role="alert">
					{error}
				</p>
			)}
		</div>
	);
}

export function BoxPage({ box, tab, csrf }: { box: string; tab: Tab; csrf: string }) {
	const { value, error, now, reload } = usePoll(() => boxGet(box), `box:${box}`);
	if (!value) {
		return (
			<div class="t-box">
				<p><a href="#/">← Tasks</a></p>
				{error ? <p class="notice danger" role="alert">{error}</p> : <p class="muted" aria-busy="true">Loading task…</p>}
			</div>
		);
	}
	const s = STATES[value.state];
	const base = `#/box/${encodeURIComponent(box)}`;
	const label: Record<Tab, ComponentChildren> = {
		timeline: 'Timeline',
		terminal: 'Terminal',
		changes: changesLabel(value.timeline),
		evidence: `Evidence ${value.artifacts.length}`,
	};
	return (
		<div class="t-box">
			<div class="t-thead">
				<div class="t-crumbs">
					<a class="t-mono" href="#/">{value.repo ?? 'tasks'}</a>
					<OtherRepos repos={value.other_repos} />
					<span class="t-sep">▸</span>
					<b>{value.title}</b>
					<span class="t-sep">▸</span>
					<span class="t-mono muted">{value.branch ?? value.box}</span>
					<span class="grow" />
					<span class={`t-pill t-${s.tone}${value.stale ? ' t-stale' : ''}`}>
						{s.glyph} {s.word}
						{value.stale && ' · stale'}
					</span>
					<HeaderActions detail={value} csrf={csrf} onDone={reload} />
				</div>
				<PrHeader box={box} named={value.pr} />
			</div>
			{error && <p class="notice danger" role="alert">{error}</p>}
			<nav class="t-tabs" role="tablist">
				{TABS.map((t) => (
					<a class="t-tab" role="tab" aria-selected={tab === t} href={t === 'timeline' ? base : `${base}/${t}`} key={t}>
						{label[t]}
					</a>
				))}
			</nav>
			<div class="t-body">
				<section>
					{value.state === 'needs_you' &&
						(value.needs?.plan ? (
							<PlanCard detail={value} needs={value.needs} csrf={csrf} onSent={reload} key={value.needs.id} />
						) : value.needs ? (
							<NeedsCard box={box} needs={value.needs} csrf={csrf} onSent={reload} key={value.needs.id} />
						) : (
							<div class="t-wait" role="region" aria-label="Your input is needed">
								<div class="t-wait-h">Your input is needed</div>
								<p class="t-wait-q">{value.line}</p>
								<p class="muted">This one has no answers to pick from here. Answer it from the terminal or from Claude Code.</p>
							</div>
						))}
					<FailedCard detail={value} csrf={csrf} onDone={reload} />
					{value.stale && (
						<p class="t-stalebar">No event for over 30 minutes. {value.line}</p>
					)}
					{tab === 'evidence' ? (
						<EvidenceTab detail={value} />
					) : tab === 'terminal' ? (
						<TerminalTab box={box} csrf={csrf} />
					) : tab === 'changes' ? (
						<ChangesTab box={box} csrf={csrf} closed={value.state === 'closed'} seen={value.last_event_at ?? value.updated_at} />
					) : (
						<Timeline detail={value} />
					)}
				</section>
				<aside class="t-rail">
					<h3>Now</h3>
					<p class="t-railline">{value.line}</p>
					<p class="muted">Last event {age(value.last_event_at ?? value.updated_at, now)}</p>
					<MakeRecurring detail={value} />
				</aside>
			</div>
			<Composer detail={value} csrf={csrf} onSent={reload} />
		</div>
	);
}

/** Where the last start went, so the next one starts there. */
const LAST_REPOS = 'moor.lastRepos';

/** The repos the last start used, first one first. */
function lastRepos(): string[] {
	try {
		const read = v.safeParse(v.array(v.string()), JSON.parse(localStorage.getItem(LAST_REPOS) ?? '[]'));
		return read.success ? read.output : [];
	} catch {
		return [];
	}
}

/** The composer that starts a Box: the footer names the target before anything is sent. */
export function NewBox({ account }: { account: Account }) {
	const [again] = useState(takeAgain);
	const [prompt, setPrompt] = useState(again?.prompt ?? '');
	const [chosen, setChosen] = useState<string[]>(() => (again?.repos.length ? again.repos : lastRepos()));
	const [base, setBase] = useState(again?.base ?? '');
	const [node, setNode] = useState('');
	const [planFirst, setPlanFirst] = useState(false);
	const [repos, setRepos] = useState<string[]>([]);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState('');
	useEffect(() => {
		repoList().then(setRepos, () => setRepos([]));
	}, []);
	const machines = account.machines.nodes.filter((n) => n.online);
	const send = async () => {
		if (busy) return;
		setBusy(true);
		setError('');
		try {
			const name = await boxStart(account.csrf, { repos: chosen, prompt, base, node, plan_first: planFirst });
			localStorage.setItem(LAST_REPOS, JSON.stringify(chosen));
			location.hash = `#/box/${encodeURIComponent(name)}`;
		} catch (e) {
			setError(e instanceof SignedOut ? 'Your session ended. Reload to sign in again.' : String((e as Error).message));
			setBusy(false);
		}
	};
	const ready = prompt.trim() !== '' && chosen.length > 0 && !busy;
	return (
		<div class="t-new">
			<h1>New task</h1>
			<form
				class="t-composer"
				onSubmit={(e) => {
					e.preventDefault();
					if (ready) void send();
				}}
			>
				<label class="t-sr" for="t-prompt">What should the agent do?</label>
				<textarea
					id="t-prompt"
					rows={6}
					placeholder="What should the agent do?"
					value={prompt}
					onInput={(e) => setPrompt(e.currentTarget.value)}
					onKeyDown={(e) => {
						if ((e.metaKey || e.ctrlKey) && e.key === 'Enter' && ready) void send();
					}}
				/>
				<div class="t-target">
					<RepoPicker repos={chosen} known={repos} onChange={setChosen} />
					<label class="t-chip">
						<span class="muted">from</span>
						<input
							class="t-mono"
							placeholder="default branch"
							value={base}
							onInput={(e) => setBase(e.currentTarget.value)}
							aria-label="Branch to start from"
						/>
					</label>
					<label class="t-chip">
						<span class="muted">on</span>
						<select value={node} onChange={(e) => setNode(e.currentTarget.value)} aria-label="Machine">
							<option value="">any free machine</option>
							{machines.map((n) => (
								<option value={n.name} key={n.name}>
									{n.name}
									{n.max_boxes != null ? ` (${Math.max(0, n.max_boxes - n.used)} free)` : ''}
								</option>
							))}
						</select>
					</label>
					<label class="t-chip" title="The agent proposes a plan and waits for your approval before it changes anything.">
						<input type="checkbox" class="pf-check" checked={planFirst} onChange={(e) => setPlanFirst(e.currentTarget.checked)} />
						Plan first
					</label>
					<span class="grow" />
					<button class="btn primary" type="submit" disabled={!ready}>
						{busy ? 'Starting…' : 'Start'} <kbd>{'⌘↵'}</kbd>
					</button>
				</div>
			</form>
			{error && <p class="notice danger" role="alert">{error}</p>}
			<p class="muted t-foot">
				The agent works on a new branch in the first repo. The others are cloned with it. You can watch it from the task page.
			</p>
		</div>
	);
}