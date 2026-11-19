import { useEffect, useState } from 'preact/hooks';

import { boxAnswer, type BoxDetail, type Needs, SignedOut } from './api';
import { planOf } from './draft';
import { gist, readPlan, type PlanDoc } from './plan';
import './plandraft.css';

/** The answers a plan-first agent offers, as the brief names them. */
const APPROVE = 'Approve plan';
const REVISE = 'Revise';

function why(e: unknown): string {
	if (e instanceof SignedOut) return 'Your session ended. Reload to sign in again.';
	return e instanceof Error ? e.message : 'Something went wrong.';
}

function Steps({ doc }: { doc: PlanDoc }) {
	return (
		<ol class="pc-steps">
			{doc.steps.map((s) => (
				<li key={s.id}>
					<strong>{s.title}</strong>
					<span class={`kind ${s.kind}`}>{s.kind}</span>
					{gist(s) && <p>{s.kind === 'check' ? <code>{gist(s)}</code> : gist(s)}</p>}
				</li>
			))}
		</ol>
	);
}

/** The card of a plan-first one-off: the plan the agent left, to approve or send back with a note. */
export function PlanCard({ detail, needs, csrf, onSent }: { detail: BoxDetail; needs: Needs; csrf: string; onSent: () => void }) {
	const [text, setText] = useState<string | null>(null);
	const [note, setNote] = useState('');
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState('');
	const newest = detail.artifacts.filter((a) => a.name === 'plan.json').reduce((n, a) => Math.max(n, a.version), 0);
	useEffect(() => {
		let live = true;
		planOf(detail).then(
			(read) => live && setText(read),
			(e: unknown) => live && setError(why(e)),
		);
		return () => {
			live = false;
		};
	}, [newest]);
	const pick = (label: string) => needs.picks.find((p) => p.label === label);
	const send = async (n: number, typed?: string) => {
		if (busy) return;
		setBusy(true);
		setError('');
		try {
			await boxAnswer(csrf, detail.box, needs.id, n, typed);
			onSent();
		} catch (e) {
			setError(why(e));
			onSent();
		} finally {
			setBusy(false);
		}
	};
	const approve = pick(APPROVE) ?? needs.picks[0];
	const revise = () => {
		const said = note.trim();
		const plain = pick(REVISE);
		if (said && needs.other != null) void send(needs.other, `${REVISE}: ${said}`);
		else if (plain) void send(plain.n);
	};
	const read = text === null ? null : readPlan(text);
	return (
		<div class="t-wait" role="region" aria-label="Approve the plan">
			<div class="t-wait-h">The agent proposes a plan{newest > 1 ? ` (version ${newest})` : ''}</div>
			<p class="t-wait-q">{needs.question}</p>
			{text === null && !error && <p class="muted" aria-busy="true">Loading the plan…</p>}
			{read && 'doc' in read && <Steps doc={read.doc} />}
			{read && 'error' in read && <pre class="t-quote">{text}</pre>}
			<div class="t-wait-btns">
				{approve && (
					<button type="button" class="t-btn" disabled={busy || text === null} onClick={() => void send(approve.n)}>
						Approve plan
					</button>
				)}
				<button type="button" class="t-btn t-btn-once" disabled={busy || (!pick(REVISE) && !note.trim())} onClick={revise}>
					Revise
				</button>
			</div>
			<label class="pc-note">
				<span class="muted">What should change? Sent with Revise.</span>
				<textarea rows={2} value={note} disabled={busy} onInput={(e) => setNote(e.currentTarget.value)} aria-label="Note for the agent" />
			</label>
			{error && (
				<p class="notice danger" role="alert">
					{error}
				</p>
			)}
		</div>
	);
}
