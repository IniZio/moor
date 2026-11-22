import type { ComponentChildren } from 'preact';
import { useState } from 'preact/hooks';

import { type BoxDetail, boxAnswer, boxGet, type Needs, type Pick, runAnswer, failure, toError } from './api';
import { ChangesTab, changesLabel } from './Changes';
import { HeaderActions } from './Ending';
import { GateCard, GateList } from './Gates';
import { STATES } from './Glyph';
import { gateLine } from './gate';
import { PlanCard, PlanRail } from './PlanCard';
import { PrHeader } from './PrHeader';
import { age, Timeline, usePoll } from './Portal';
import { FailedCard } from './Retry';
import { useDraft } from './session';
import { Composer } from './Steer';
import { MakeRecurring } from './MakeRecurring';
import { TerminalTab } from './Terminal';

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
	return needs.gate ? (
		<GateCard box={box} needs={needs} wait={needs.gate} csrf={csrf} onSent={onSent} />
	) : needs.kind === 'approve' && needs.approve ? (
		<ApproveCard needs={needs} run={needs.approve} csrf={csrf} onSent={onSent} />
	) : (
		<AskCard box={box} needs={needs} csrf={csrf} onSent={onSent} />
	);
}

/** An agent's own prompt: a permission or a question. */
function AskCard({ box, needs, csrf, onSent }: { box: string; needs: Needs; csrf: string; onSent: () => void }) {
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState('');
	const [typed, setTyped, dropTyped] = useDraft(`ask:${box}:${needs.id}`);
	const send = async (n: number, text?: string) => {
		if (busy) return;
		setBusy(true);
		setError('');
		try {
			await boxAnswer(csrf, box, needs.id, n, text);
			dropTyped();
			onSent();
		} catch (e) {
			setError(failure(toError(e)));
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
	const [note, setNote, dropNote] = useDraft(`approve:${run.workflow}:${run.run}`);
	const send = async (yes: boolean) => {
		if (busy) return;
		setBusy(true);
		setError('');
		try {
			await runAnswer(csrf, run.workflow, run.run, yes, note);
			dropNote();
			onSent();
		} catch (e) {
			setError(failure(toError(e)));
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
						<ChangesTab box={box} seen={value.last_event_at ?? value.updated_at} />
					) : (
						<Timeline detail={value} />
					)}
				</section>
				<aside class="t-rail">
					<h3>Now</h3>
					<p class="t-railline">{(value.state === 'needs_you' || value.failed ? gateLine(value.gate) : null) ?? value.line}</p>
					<p class="muted">Last event {age(value.last_event_at ?? value.updated_at, now)}</p>
					<GateList gate={value.gate} />
					<PlanRail detail={value} />
					<MakeRecurring detail={value} />
				</aside>
			</div>
			<Composer detail={value} csrf={csrf} onSent={reload} />
		</div>
	);
}
