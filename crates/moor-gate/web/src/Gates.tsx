import { useState } from 'preact/hooks';

import { failure, gateAnswer, type Needs, toError } from './api';
import { checkRun, closingRow, GATE_LOOK, type GateDoc, type GateView, gateRows, type OutcomeDoc, quoted, ruleSentence, tailView, type Tried, type Wait, whyWords } from './gate';
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
	/** Whether the person pressed No and is writing the note that goes with it. */
	refusing: boolean;
	note: string;
	error: string;
	onYes: () => void;
	onNo: () => void;
	onSendNo: () => void;
	onBack: () => void;
	onNote: (text: string) => void;
};

/** What a gate that asks its owner shows: what is asked, what the agent handed in and wrote, and Yes or No. */
export function ApproveView({ box, wait, busy, refusing, note, error, onYes, onNo, onSendNo, onBack, onNote }: ApproveProps) {
	return (
		<div class="t-wait" role="region" aria-label="Your approval is needed">
			<div class="t-wait-h">Your approval is needed</div>
			<p class="t-wait-q">The agent asks you to approve {quoted(wait.gate)}.</p>
			{wait.handed.length > 0 && (
				<p class="t-gate-handed">
					<span class="t-gate-label">The agent handed in</span>
					{wait.handed.map((name) => (
						<a class="t-mono" href={evidenceHref(box)} key={name} title="Open it in Evidence">
							{name}
						</a>
					))}
				</p>
			)}
			{wait.note && (
				<>
					<p class="t-gate-label">The agent wrote</p>
					<blockquote class="t-gate-note">{wait.note}</blockquote>
				</>
			)}
			<p class="muted t-wait-note">Yes lets the task go on. No sends the agent back to try again, with your note.</p>
			{refusing ? (
				<form
					class="t-wait-own"
					onSubmit={(e) => {
						e.preventDefault();
						onSendNo();
					}}
				>
					<input
						value={note}
						onInput={(e) => onNote(e.currentTarget.value)}
						placeholder="Tell the agent what to change (a short note)"
						aria-label="Note for the agent"
						disabled={busy}
					/>
					<button type="submit" class="t-btn t-btn-deny" disabled={busy}>
						Send No
					</button>
					<button type="button" class="t-btn t-btn-once" disabled={busy} onClick={onBack}>
						Back
					</button>
				</form>
			) : (
				<div class="t-wait-btns">
					<button type="button" class="t-btn" disabled={busy} onClick={onYes}>
						Yes
					</button>
					<button type="button" class="t-btn t-btn-deny" disabled={busy} onClick={onNo}>
						No
					</button>
				</div>
			)}
			{error && (
				<p class="notice danger" role="alert">
					{error}
				</p>
			)}
		</div>
	);
}

/** The card of a gate that asks its owner: Yes passes it, No fails the try and the note goes to the agent. */
function ApproveCard({ box, needs, wait, csrf, onSent }: { box: string; needs: Needs; wait: Wait; csrf: string; onSent: () => void }) {
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState('');
	const [refusing, setRefusing] = useState(false);
	const [note, setNote, dropNote] = useDraft(`gate:${box}:${needs.id}`);
	const send = async (yes: boolean) => {
		if (busy) return;
		setBusy(true);
		setError('');
		try {
			await gateAnswer(csrf, box, needs.id, yes, yes ? undefined : note);
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
		<ApproveView
			box={box}
			wait={wait}
			busy={busy}
			refusing={refusing}
			note={note}
			error={error}
			onYes={() => void send(true)}
			onNo={() => setRefusing(true)}
			onSendNo={() => void send(false)}
			onBack={() => setRefusing(false)}
			onNote={setNote}
		/>
	);
}

/** The outcomes moor tried at the closing stage, each with what it says, how its check ended, what it printed and the command behind a click. */
function TriedList({ tried }: { tried: readonly Tried[] }) {
	return (
		<ul class="t-gate-tried">
			{tried.map((t) => (
				<li key={t.outcome}>
					<p class="t-gate-says">{t.says ?? t.outcome}</p>
					<p class="t-gate-why">{t.exit == null ? 'Its output was lost.' : `Exited with code ${t.exit}.`}</p>
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

/** What a stuck gate shows: why, the check that failed with its output, or the evidence not found, and what a person can do. The message box is the page's own, below. */
export function StuckView({ wait }: { wait: Wait }) {
	return (
		<div class="t-wait" role="region" aria-label="This gate is stuck">
			<div class="t-wait-h">Stuck, and needs you</div>
			<p class="t-wait-q">{wait.tried?.length ? 'No outcome holds yet.' : `${quoted(wait.gate)} is stuck.`}</p>
			{wait.tried?.length ? (
				<>
					<p class="t-gate-why">{whyWords(wait.why ?? 'it cannot go on')}</p>
					<TriedList tried={wait.tried} />
				</>
			) : null}
			{!wait.tried?.length && wait.says && (
				<>
					<p class="t-gate-label">Not yet true</p>
					<p class="t-gate-says">{wait.says}</p>
				</>
			)}
			{!wait.tried?.length && <p class="t-gate-why">{whyWords(wait.why ?? 'it cannot go on', wait.says)}</p>}
			{wait.command && !wait.says && !wait.tried?.length && (
				<>
					<p class="t-gate-label">The check that failed</p>
					<pre class="t-quote">{wait.command}</pre>
				</>
			)}
			{wait.tail && !wait.tried?.length && (
				<>
					<p class="t-gate-label">What it printed</p>
					<Tail text={wait.tail} />
				</>
			)}
			{wait.command && wait.says && !wait.tried?.length && (
				<details class="t-gate-cmd">
					<summary>Show the check</summary>
					<pre class="t-quote">{wait.command}</pre>
				</details>
			)}
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
			<p class="muted t-wait-note">What you can do: write to the agent below. Your message reopens this gate with fresh tries.</p>
		</div>
	);
}

/** The card for a gate that waits for a person. */
export function GateCard({ box, needs, wait, csrf, onSent }: { box: string; needs: Needs; wait: Wait; csrf: string; onSent: () => void }) {
	return wait.kind === 'approve' ? <ApproveCard box={box} needs={needs} wait={wait} csrf={csrf} onSent={onSent} /> : <StuckView wait={wait} />;
}

/** The gates of a Box by name, each with where it stands. */
export function GateList({ gate }: { gate: GateView | null | undefined }) {
	if (!gate || gate.gates.length === 0) return null;
	const rows = [...gateRows(gate), ...[closingRow(gate)].filter((r) => r !== null)];
	return (
		<>
			<h3>Gates</h3>
			<ol class="t-gates">
				{rows.map((r) => (
					<li class={`t-gate t-${GATE_LOOK[r.state].tone}`} key={r.name}>
						<span class="t-gate-glyph" aria-hidden="true">
							{GATE_LOOK[r.state].glyph}
						</span>
						<span class="t-gate-name t-mono">{r.name}</span>
						<span class="t-gate-words">{r.words}</span>
					</li>
				))}
			</ol>
		</>
	);
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
					<textarea aria-label="Gates JSON" class="t-mono" rows={14} spellcheck={false} readOnly value={JSON.stringify(gates, null, 2)} />
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
