import type { ComponentChildren } from 'preact';
import { useState } from 'preact/hooks';

import { type BoxDetail, boxAnswer, boxGet, type Needs, type Pick, runAnswer, failure, toError, workflowGet } from './api';
import { AskedTask, askedPollMs } from './AskedTask';
import { ChangesTab, changesLabel } from './Changes';
import { HeaderActions } from './Ending';
import { GateCard } from './Gates';
import { GateStory, PromptFold } from './GateStory';
import { OutputList } from './PlaceCards';
import { PlanCard, PlanRail } from './PlanCard';
import { handedImages } from './Peek';
import { PrHeader } from './PrHeader';
import { Timeline, usePoll } from './Portal';
import { FailedCard } from './Retry';
import { useDraft } from './session';
import { Composer } from './Steer';
import { MakeRecurring } from './MakeRecurring';
import { TaskHead } from './TaskHead';
import { TerminalTab } from './Terminal';
import { useLoad } from './useLoad';
import './story.css';

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

const TABS = ['timeline', 'evidence', 'changes', 'terminal'] as const;
export type Tab = (typeof TABS)[number];

/** The tabs the row shows: the terminal sits under the more menu, beside Close. */
const SHOWN_TABS: readonly Tab[] = ['timeline', 'evidence', 'changes'];

/** The tabs a reader of another person's task gets: the terminal writes and the changes are read live in the owner's Box. */
const READ_TABS: readonly Tab[] = ['timeline', 'evidence'];

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
function NeedsCard({ detail, needs, csrf, onSent }: { detail: BoxDetail; needs: Needs; csrf: string; onSent: () => void }) {
	const box = detail.box;
	return needs.gate ? (
		<GateCard box={box} needs={needs} wait={needs.gate} csrf={csrf} onSent={onSent} images={handedImages(needs.gate.handed, detail)} pr={detail.pr?.number ?? null} />
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

export function BoxPage({ box, tab, csrf, me }: { box: string; tab: Tab; csrf: string; me: string }) {
	const { value, error, now, reload } = usePoll(() => boxGet(box), `box:${box}`, askedPollMs);
	if (!value) {
		return (
			<div class="t-box">
				<p><a href="#/">← Tasks</a></p>
				{error ? <p class="notice danger" role="alert">{error}</p> : <p class="muted" aria-busy="true">Loading task…</p>}
			</div>
		);
	}
	return <TaskPage detail={value} error={error} now={now} tab={tab} csrf={csrf} me={me} reload={reload} />;
}

/** The plain sentence per gate lives in the workflow that made the Box; a one-off has none, and a workflow that cannot be read leaves the story to names and states. */
function useWorkflowDocs(name: string | undefined) {
	return useLoad(() => (name ? workflowGet(name).catch(() => null) : Promise.resolve(null)), `flow:${name ?? ''}`).value;
}

function TaskPage({ detail, error, now, tab, csrf, me, reload }: { detail: BoxDetail; error: string; now: number; tab: Tab; csrf: string; me: string; reload: () => void }) {
	const docs = useWorkflowDocs(detail.workflow?.name);
	const box = detail.box;
	const base = `#/box/${encodeURIComponent(box)}`;
	const label: Record<Tab, ComponentChildren> = {
		timeline: 'Timeline',
		terminal: 'Terminal',
		changes: changesLabel(detail.timeline),
		evidence: detail.artifacts.length ? (
			<>
				Evidence <span class="t-cnt">{detail.artifacts.length}</span>
			</>
		) : (
			'Evidence'
		),
	};
	const { needs, owner } = detail;
	const asked = detail.timeline.find((e) => e.class === 'you')?.at;
	if (owner) return <ReadOnlyTask detail={detail} owner={owner} error={error} now={now} tab={tab} base={base} label={label} docs={docs} />;
	const card =
		detail.state !== 'needs_you' ? null : needs?.plan ? (
			<PlanCard detail={detail} needs={needs} csrf={csrf} onSent={reload} key={needs.id} />
		) : needs ? (
			<NeedsCard detail={detail} needs={needs} csrf={csrf} onSent={reload} key={needs.id} />
		) : (
			<div class="t-wait" role="region" aria-label="Your input is needed">
				<div class="t-wait-h">Your input is needed</div>
				<p class="t-wait-q">{detail.line}</p>
				<p class="muted">This one has no answers to pick from here. Answer it from the terminal or from Claude Code.</p>
			</div>
		);
	if (detail.task_at || detail.workflow) return <AskedTask detail={detail} error={error} now={now} csrf={csrf} reload={reload} card={card} you={me} tab={tab} docs={docs} />;
	return (
		<div class="t-box">
			<TaskHead
				t={detail}
				now={now}
				asked={asked}
				actions={<HeaderActions part="stop" detail={detail} csrf={csrf} onDone={reload} />}
				pr={<PrHeader box={box} named={detail.pr} listed={detail.pr?.number ?? null} />}
			/>
			{error && <p class="notice danger" role="alert">{error}</p>}
			{card}
			<FailedCard detail={detail} csrf={csrf} onDone={reload} />
			{detail.stale && <p class="t-stalebar">No event for over 30 minutes. {detail.line}</p>}
			{detail.gate?.gates.length ? <GateStory gate={detail.gate} docs={docs} card={null} wait={detail.state === 'needs_you' ? needs?.gate : null} /> : null}
			<PromptFold prompt={detail.prompt} />
			<OutputList outputs={detail.outputs} />
			<nav class="t-tabs" role="tablist">
				{SHOWN_TABS.map((t) => (
					<a class="t-tab" role="tab" aria-selected={tab === t} href={t === 'timeline' ? base : `${base}/${t}`} key={t}>
						{label[t]}
					</a>
				))}
				<span class="grow" />
				<HeaderActions part="more" detail={detail} csrf={csrf} onDone={reload} />
			</nav>
			<section>
				{tab === 'evidence' ? (
					<EvidenceTab detail={detail} />
				) : tab === 'terminal' ? (
					<TerminalTab box={box} csrf={csrf} />
				) : tab === 'changes' ? (
					<ChangesTab box={box} seen={detail.last_event_at ?? detail.updated_at} />
				) : (
					<Timeline detail={detail} />
				)}
			</section>
			<div class="t-rail">
				<PlanRail detail={detail} />
				<MakeRecurring detail={detail} />
			</div>
			<Composer detail={detail} csrf={csrf} onSent={reload} />
		</div>
	);
}

/** Another person's task, to read: its head, story, timeline and evidence, and no control that acts on it. */
function ReadOnlyTask({
	detail,
	owner,
	error,
	now,
	tab,
	base,
	label,
	docs,
}: {
	detail: BoxDetail;
	owner: string;
	error: string;
	now: number;
	tab: Tab;
	base: string;
	label: Record<Tab, ComponentChildren>;
	docs: ReturnType<typeof useWorkflowDocs>;
}) {
	const shown = READ_TABS.includes(tab) ? tab : 'timeline';
	return (
		<div class="t-box">
			<TaskHead t={detail} now={now} asked={detail.timeline.find((e) => e.class === 'you')?.at} actions={<span class="t-readonly">{owner}'s task</span>} />
			<p class="t-readonly-note muted">You can read this task. Only {owner} can answer, steer or close it.</p>
			{error && <p class="notice danger" role="alert">{error}</p>}
			{detail.gate?.gates.length ? <GateStory gate={detail.gate} docs={docs} card={null} /> : null}
			<PromptFold prompt={detail.prompt} />
			<OutputList outputs={detail.outputs} />
			<nav class="t-tabs" role="tablist">
				{READ_TABS.map((t) => (
					<a class="t-tab" role="tab" aria-selected={shown === t} href={t === 'timeline' ? base : `${base}/${t}`} key={t}>
						{label[t]}
					</a>
				))}
			</nav>
			<section>{shown === 'evidence' ? <EvidenceTab detail={detail} /> : <Timeline detail={detail} />}</section>
		</div>
	);
}
