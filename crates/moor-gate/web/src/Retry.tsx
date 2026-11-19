import { useState } from 'preact/hooks';

import { type BoxDetail, boxRetry, SignedOut, workflowRunNow } from './api';

/** What the composer opens with when a person starts a failed Box again. */
export type Draft = { repo: string; base: string; prompt: string };

const AGAIN = 'moor.again';

/** Opens the composer with the same repo, base and prompt as a Box that failed. */
function startAgain(draft: Draft) {
	sessionStorage.setItem(AGAIN, JSON.stringify(draft));
	location.hash = '#/new';
}

/** The draft a start again left for the composer, taken once so a later visit opens empty. */
export function takeAgain(): Draft | null {
	const raw = sessionStorage.getItem(AGAIN);
	sessionStorage.removeItem(AGAIN);
	try {
		const draft = raw ? (JSON.parse(raw) as Partial<Draft>) : null;
		return draft ? { repo: draft.repo ?? '', base: draft.base ?? '', prompt: draft.prompt ?? '' } : null;
	} catch {
		return null;
	}
}

/** The prompt the person wrote: a workflow run's first message holds it alone, apart from the agent's instructions. */
function promptOf(detail: BoxDetail): string {
	return detail.timeline.find((e) => e.step)?.text ?? detail.prompt ?? '';
}

/** Whether the Box ended badly: it failed, or it closed after an error. */
function failedBox(detail: BoxDetail): boolean {
	return detail.state === 'failed' || (detail.state === 'closed' && detail.timeline.some((e) => e.class === 'error'));
}

/** What a person can do about a Box that failed: run its setup again, start it again, or run its workflow again. */
export function FailedCard({ detail, csrf, onDone }: { detail: BoxDetail; csrf: string; onDone: () => void }) {
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState('');
	const failed = failedBox(detail);
	const flow = detail.workflow;
	if (!failed && !flow) return null;
	const act = async (go: () => Promise<void>) => {
		if (busy) return;
		setBusy(true);
		setError('');
		try {
			await go();
			onDone();
		} catch (e) {
			setError(e instanceof SignedOut ? 'Your session ended. Reload to sign in again.' : String((e as Error).message));
		} finally {
			setBusy(false);
		}
	};
	const runAgain = async () => {
		if (!flow) return;
		const started = await workflowRunNow(csrf, flow.name, flow.repo);
		if (started.box) location.hash = `#/box/${encodeURIComponent(started.box)}`;
	};
	return (
		<div class="t-fail" role="region" aria-label="This task failed">
			<div class="t-fail-h">{failed ? 'This task failed' : 'The workflow run behind this task failed'}</div>
			<p class="t-fail-q">{detail.line}</p>
			<div class="t-wait-btns">
				{detail.retryable && (
					<button type="button" class="t-btn" disabled={busy} onClick={() => void act(() => boxRetry(csrf, detail.box))}>
						{busy ? 'Retrying…' : 'Retry setup'}
					</button>
				)}
				{failed && (
					<button
						type="button"
						class="t-btn t-btn-once"
						disabled={busy}
						onClick={() => startAgain({ repo: detail.repo ?? '', base: detail.base ?? '', prompt: promptOf(detail) })}
					>
						Start again
					</button>
				)}
				{flow && (
					<button type="button" class="t-btn t-btn-once" disabled={busy} onClick={() => void act(runAgain)}>
						Run again
					</button>
				)}
			</div>
			{error && (
				<p class="notice danger" role="alert">
					{error}
				</p>
			)}
		</div>
	);
}
