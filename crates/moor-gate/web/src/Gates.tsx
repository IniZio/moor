import { useState } from 'preact/hooks';

import { failure, gateAnswer, type Needs, toError } from './api';
import { GATE_LOOK, type GateView, gateRows, quoted, tailView, type Wait, whyWords } from './gate';
import { useDraft } from './session';
import './gates.css';

/** Where a task's evidence is shown. */
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

/** What a stuck gate shows: why, the check that failed with its output, or the evidence not found, and what a person can do. The message box is the page's own, below. */
export function StuckView({ wait }: { wait: Wait }) {
	return (
		<div class="t-wait" role="region" aria-label="This gate is stuck">
			<div class="t-wait-h">Stuck, and needs you</div>
			<p class="t-wait-q">{quoted(wait.gate)} is stuck.</p>
			<p class="t-gate-why">{whyWords(wait.why ?? 'it cannot go on')}</p>
			{wait.command && (
				<>
					<p class="t-gate-label">The check that failed</p>
					<pre class="t-quote">{wait.command}</pre>
				</>
			)}
			{wait.tail && (
				<>
					<p class="t-gate-label">What it printed</p>
					<Tail text={wait.tail} />
				</>
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

/** The gates of a task by name, each with where it stands. */
export function GateList({ gate }: { gate: GateView | null | undefined }) {
	if (!gate || gate.gates.length === 0) return null;
	return (
		<>
			<h3>Gates</h3>
			<ol class="t-gates">
				{gateRows(gate).map((r) => (
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
