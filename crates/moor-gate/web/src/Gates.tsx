import type { ComponentChildren } from 'preact';
import { useState } from 'preact/hooks';

import { type Evidence, failure, gateAnswer, type Needs, toError } from './api';
import { boxSteer } from './steerApi';
import { checkRun, checkWords, CLOSING, type GateDoc, UNANSWERED_HEAD, type OutcomeDoc, quoted, ruleSentence, tailView, REOPEN_UNANSWERED, type Tried, type Unanswered, unansweredWhy, type Wait, whyWords } from './gate';
import { useDraft } from './session';
import './gates.css';

/** Where a Box's evidence is shown. */
const evidenceHref = (box: string) => `#/box/${encodeURIComponent(box)}/evidence`;

/** A check's output in monospace: its last lines, and the rest behind a click. */
export function Tail({ text }: { text: string }) {
	const { shown, all } = tailView(text);
	return (
		<div class="t-gate-tail">
			<pre class="t-quote">{shown}</pre>
			{all !== null && (
				<details>
					<summary>Show the whole output</summary>
					<pre class="t-quote">{all}</pre>
				</details>
			)}
		</div>
	);
}

export type ApproveProps = {
	box: string;
	wait: Wait;
	busy: boolean;
	note: string;
	error: string;
	/** The images among what the agent handed in, to show as thumbnails. */
	images: readonly Evidence[];
	/** The pull request the task opened, named after what the agent wrote. */
	pr: number | null;
	onYes: () => void;
	onNo: () => void;
	onNote: (text: string) => void;
};

/** One thing the agent handed in: its picture when it is an image, else its name, either opening it in Evidence. */
function Handed({ box, name, image }: { box: string; name: string; image?: Evidence }) {
	return (
		<figure>
			<a href={image ? image.href : evidenceHref(box)} target={image ? '_blank' : undefined} rel={image ? 'noopener' : undefined} title={name}>
				{image ? <img class="t-thumb" src={image.href} alt={name} loading="lazy" /> : <span class="t-thumb t-thumb-file" aria-hidden="true" />}
			</a>
			<figcaption>{name}</figcaption>
		</figure>
	);
}

/** What a gate that asks its owner shows: what the agent handed in and wrote, a note for the agent, and yes or no. */
export function ApproveView({ box, wait, busy, note, error, images, pr, onYes, onNo, onNote }: ApproveProps) {
	return (
		<section class="t-ncard" aria-label="Needs you">
			<h2>Waiting for your yes on {quoted(wait.gate)}</h2>
			{(wait.handed.length > 0 || wait.note) && (
				<>
					{wait.handed.length > 0 && <p>The agent handed in:</p>}
					<div class="t-thumbs">
						{wait.handed.map((name) => (
							<Handed box={box} name={name} image={images.find((a) => a.name === name)} key={name} />
						))}
						{wait.note && (
							<blockquote class="t-anote">
								{wait.note}
								{pr !== null && ` PR #${pr} is open.`}
							</blockquote>
						)}
					</div>
				</>
			)}
			<textarea class="t-ynote" rows={1} value={note} placeholder="Add a note for the agent (optional)" aria-label="Note for the agent" disabled={busy} onInput={(e) => onNote(e.currentTarget.value)} />
			<div class="t-actions">
				<button type="button" class="btn primary" disabled={busy} onClick={onYes}>
					Yes, it’s done
				</button>
				<button type="button" class="btn" disabled={busy} onClick={onNo}>
					No, send it back
				</button>
			</div>
			{error && (
				<p class="notice danger" role="alert">
					{error}
				</p>
			)}
		</section>
	);
}

/** The state and handlers of the card for a gate that asks its owner: Yes passes it, No fails the try and the note goes to the agent. The task page and the peek panel share it. */
export function useApproval(box: string, needs: Needs, wait: Wait, csrf: string, onSent: () => void, images: readonly Evidence[], pr: number | null): ApproveProps {
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState('');
	const [note, setNote, dropNote] = useDraft(`gate:${box}:${needs.id}`);
	const send = async (yes: boolean) => {
		if (busy) return;
		setBusy(true);
		setError('');
		try {
			await gateAnswer(csrf, box, needs.id, yes, note.trim() || undefined);
			dropNote();
			onSent();
		} catch (e) {
			setError(failure(toError(e)));
			onSent();
		} finally {
			setBusy(false);
		}
	};
	return {
		box,
		wait,
		busy,
		note,
		error,
		images,
		pr,
		onYes: () => void send(true),
		onNo: () => void send(false),
		onNote: setNote,
	};
}

type CardProps = { box: string; needs: Needs; wait: Wait; csrf: string; onSent: () => void; images?: readonly Evidence[]; pr?: number | null; inPeek?: boolean };

/** The card for a stuck gate, with one more try and the terminal a click away. */
function StuckCard({ box, wait, csrf, onSent, inPeek }: CardProps) {
	return <StuckView wait={wait} inPeek={inPeek} retry={useRetry(box, wait, csrf, onSent)} terminal={inPeek ? undefined : `#/box/${encodeURIComponent(box)}/terminal`} />;
}

function ApproveCard({ box, needs, wait, csrf, onSent, images = [], pr = null }: CardProps) {
	return <ApproveView {...useApproval(box, needs, wait, csrf, onSent, images, pr)} />;
}

/** The outcomes moor tried at the closing stage, each with what it says, how its check ended, what it printed and the command behind a click. */
function TriedList({ tried }: { tried: readonly Tried[] }) {
	return (
		<ul class="t-gate-tried">
			{tried.map((t) => (
				<li key={t.outcome}>
					<p class="t-gate-says">{t.says ?? t.outcome}</p>
					<p class="t-gate-why">{`Exited with code ${t.exit}.`}</p>
					{t.tail.trim() !== '' && (
						<>
							<p class="t-gate-label">What it printed</p>
							<Tail text={t.tail} />
						</>
					)}
					{t.command && (
						<details class="t-gate-cmd">
							<summary>Show the check</summary>
							<pre class="t-quote">{t.command}</pre>
						</details>
					)}
				</li>
			))}
		</ul>
	);
}

/** The frame of a card for a stuck gate: what is wrong as its heading, then the body, then what the person can do. */
function StuckFrame({ head, tone, note, children, actions }: { head: string; tone: 'stuck' | 'trouble'; note: string; children: ComponentChildren; actions?: ComponentChildren }) {
	return (
		<section class={`t-ncard t-ncard-${tone}`} aria-label="Needs you">
			<h2>{head}</h2>
			{children}
			<p>{note}</p>
			{actions}
		</section>
	);
}

/** What the person can do about a stuck gate: the message box is on the task page, below the card there and a click away in the peek panel. */
const reopen = (inPeek?: boolean) => `You can write to the agent${inPeek ? ' on the task page' : ' below'}, open the terminal and fix it yourself, or give it one more try.`;

/** One more try for a stuck gate: a steer reopens it with fresh tries, as a message from the box below would. */
export type Retry = { busy: boolean; error: string; onRetry: () => void };

/** Gives a stuck gate one more try: a steer to the agent, which reopens the gate with fresh tries. */
export function retryGate(csrf: string, box: string, gate: string) {
	return boxSteer(csrf, box, `Give ${quoted(gate)} one more try.`, false);
}

/** The state and handler of "Give it one more try". */
export function useRetry(box: string, wait: Wait, csrf: string, onSent: () => void): Retry {
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState('');
	const onRetry = async () => {
		if (busy) return;
		setBusy(true);
		setError('');
		try {
			await retryGate(csrf, box, wait.gate);
		} catch (e) {
			setError(failure(toError(e)));
		} finally {
			setBusy(false);
			onSent();
		}
	};
	return { busy, error, onRetry: () => void onRetry() };
}

const TRIES = ['no', 'one', 'two', 'three', 'four', 'five'];

/** A check moor got no answer from: moor's trouble, not the agent's, with the check, why, and what the person can do. */
function NoAnswerView({ gate, cause, hint }: { gate: string; cause: Unanswered; hint: string | null | undefined }) {
	return (
		<StuckFrame head={gate === CLOSING ? UNANSWERED_HEAD : `moor could not run ${quoted(gate)}`} tone="trouble" note={hint ?? REOPEN_UNANSWERED}>
			<p>This is moor’s trouble, not the agent’s. moor tried twice. {unansweredWhy(cause)}</p>
			{cause.says ? <p class="t-gate-says">{cause.says}</p> : cause.command && <pre class="t-quote">{cause.command}</pre>}
			{cause.says && cause.command && (
				<details class="t-gate-cmd">
					<summary>Show the check</summary>
					<pre class="t-quote">{cause.command}</pre>
				</details>
			)}
			<p class="muted">The agent is waiting. Nothing it did is lost.</p>
		</StuckFrame>
	);
}

/** The sentence that says what the gate holds the agent to and what moor ran. */
function Ran({ wait }: { wait: Wait }) {
	const tries = wait.try_n && wait.try_n > 1 ? ` after each of the agent’s ${TRIES[wait.try_n] ?? wait.try_n} tries` : '';
	if (!wait.says && !wait.command) return null;
	return (
		<p>
			{wait.says && (
				<>
					What must be true: <b>{/[.!?]$/.test(wait.says) ? wait.says : `${wait.says}.`}</b>{' '}
				</>
			)}
			{wait.command && (
				<>
					moor ran <code>{wait.command}</code>
					{tries}.
				</>
			)}
		</p>
	);
}

/** What a stuck gate shows: why, the check that failed with its output, or the evidence not found, and what a person can do: one more try, and on the task page the terminal. The message box is the page's own. */
export function StuckView({ wait, inPeek, retry, terminal }: { wait: Wait; inPeek?: boolean; retry?: Retry; terminal?: string }) {
	if (wait.unanswered) return <NoAnswerView gate={wait.gate} cause={wait.unanswered} hint={wait.hint} />;
	if (wait.tried?.length) {
		return (
			<StuckFrame head="No outcome holds." tone="stuck" note={reopen(inPeek)}>
				<p class="t-gate-why">moor tried each outcome twice.</p>
				<TriedList tried={wait.tried} />
			</StuckFrame>
		);
	}
	return (
		<StuckFrame
			head={`Stuck on ${quoted(wait.gate)}`}
			tone="stuck"
			note={reopen(inPeek)}
			actions={
				(retry || terminal) && (
					<>
						<div class="t-actions">
							{retry && (
								<button type="button" class="btn primary" disabled={retry.busy} onClick={retry.onRetry}>
									Give it one more try
								</button>
							)}
							{terminal && (
								<a class="btn" href={terminal}>
									Open the terminal
								</a>
							)}
						</div>
						{retry?.error && (
							<p class="notice danger" role="alert">
								{retry.error}
							</p>
						)}
					</>
				)
			}
		>
			<Ran wait={wait} />
			<p class="t-exit">{whyWords(wait.why ?? 'it cannot go on', wait.says).replace(/\.$/, '')}</p>
			{wait.tail && <Tail text={wait.tail} />}
			{wait.missing.length > 0 && (
				<>
					<p class="t-gate-label">The agent did not hand in</p>
					<ul class="t-gate-missing t-mono">
						{wait.missing.map((name) => (
							<li key={name}>{name}</li>
						))}
					</ul>
				</>
			)}
		</StuckFrame>
	);
}

/** The card for a gate that waits for a person. */
export function GateCard(p: CardProps) {
	return p.wait.kind === 'approve' ? <ApproveCard {...p} /> : <StuckCard {...p} />;
}

export type RulesProps = { gates: readonly GateDoc[]; outcomes?: readonly OutcomeDoc[] | null; asJson: boolean; onToggle: () => void };

/** How a run may end: each outcome with what it says, marked when it can end the run at any point or holds when no other does. */
function OutcomeRules({ outcomes }: { outcomes: readonly OutcomeDoc[] }) {
	return (
		<>
			<p class="t-rules-end">A run ends as one of</p>
			<ul class="t-rules-list">
				{outcomes.map((o) => (
					<li key={o.name}>
						<span class="t-rule-name">{o.name}</span>
						{o.says ? ` — ${o.says}` : ''}
						{o.anytime && <span class="muted"> · at any point</span>}
						{(o.checks ?? []).length === 0 && <span class="muted"> · when no other holds</span>}
						{(o.checks ?? []).map((c) => (
							<div class="t-rule-check" key={checkRun(c)}>
								moor checks: {checkWords(c)}
								<details class="t-gate-cmd">
									<summary>Show the check</summary>
									<pre class="t-quote">{checkRun(c)}</pre>
								</details>
							</div>
						))}
					</li>
				))}
			</ul>
		</>
	);
}

/** What moor holds a gated run to: each gate in order, in sentences, with the commands behind a click. Read-only. */
export function RulesView({ gates, outcomes, asJson, onToggle }: RulesProps) {
	return (
		<div class="t-rules" role="region" aria-label="What moor holds this run to">
			<div class="plan-bar">
				<strong class="grow">What moor holds this run to</strong>
				<button type="button" class="quiet" aria-pressed={asJson} onClick={onToggle}>
					Edit as JSON
				</button>
			</div>
			{asJson ? (
				<>
					<textarea aria-label="Gates JSON" class="t-mono" rows={14} spellcheck={false} readOnly value={JSON.stringify(outcomes?.length ? { gates, outcomes } : gates, null, 2)} />
					<p class="muted">Read-only here. Change the gates with add_workflow.</p>
				</>
			) : (
				<>
					<ol class="t-rules-list">
						{gates.map((gate, i) => (
							<li key={gate.name}>
								<span class="t-rule-name">
									{i + 1}. {gate.name}
								</span>
								{' — '}
								{ruleSentence(gate)}
								{(gate.checks ?? []).length > 0 && (
									<details class="t-gate-cmd">
										<summary>{gate.checks?.length === 1 ? 'Show the check' : 'Show the checks'}</summary>
										<pre class="t-quote">{(gate.checks ?? []).map(checkRun).join('\n\n')}</pre>
									</details>
								)}
							</li>
						))}
					</ol>
					{outcomes && outcomes.length > 0 ? <OutcomeRules outcomes={outcomes} /> : <p class="t-rules-end">Then the run is delivered.</p>}
				</>
			)}
		</div>
	);
}

/** The read-only panel of a gated workflow's gates, with its JSON behind a toggle. */
export function GatesPanel({ gates, outcomes }: { gates: readonly GateDoc[]; outcomes?: readonly OutcomeDoc[] | null }) {
	const [asJson, setAsJson] = useState(false);
	return <RulesView gates={gates} outcomes={outcomes} asJson={asJson} onToggle={() => setAsJson(!asJson)} />;
}
