import { useEffect, useRef, useState } from 'preact/hooks';

import { boxGet, SignedOut } from './api';
import { boxClose, planDraft, planOf } from './draft';
import './plandraft.css';

/** Where a draft stands. */
type Draft =
	| { at: 'idle'; note?: string }
	| { at: 'running'; box: string; line: string; quietSince?: number }
	| { at: 'failed'; text: string; box?: string };

/** How often the page reads the drafting Box. */
const EVERY_MS = 3000;

/** How long a quiet Box may go without leaving a plan before the page gives up on it. */
const GRACE_MS = 60_000;

function why(e: unknown): string {
	if (e instanceof SignedOut) return 'Your session ended. Reload to sign in again.';
	return e instanceof Error ? e.message : 'Something went wrong.';
}

/** Generate plan: a short Box reads the repos and drafts a plan for the prompt, which lands in the editor for review. */
export function GeneratePlan({ csrf, repos, prompt, onPlan }: { csrf: string; repos: string[]; prompt: string; onPlan: (text: string) => void }) {
	const [draft, setDraft] = useState<Draft>({ at: 'idle' });
	const live = useRef(true);
	useEffect(
		() => () => {
			live.current = false;
		},
		[],
	);
	const running = draft.at === 'running' ? draft : null;
	useEffect(() => {
		if (!running?.box) return;
		const timer = setTimeout(() => void look(running), EVERY_MS);
		return () => clearTimeout(timer);
	}, [running]);
	const look = async (now: { box: string; quietSince?: number }) => {
		try {
			const detail = await boxGet(now.box);
			const text = await planOf(detail);
			if (!live.current) return;
			if (text !== null) {
				onPlan(text);
				setDraft({ at: 'idle', note: 'The agent drafted this plan. Review the steps, then save.' });
				await boxClose(csrf, now.box).catch(() => undefined);
				return;
			}
			if (detail.state === 'failed' || detail.state === 'closed') {
				return setDraft({ at: 'failed', text: `The agent stopped without a plan: ${detail.line}`, box: now.box });
			}
			const quiet = detail.state === 'done' || detail.state === 'asleep' || detail.state === 'needs_you';
			const quietSince = quiet ? (now.quietSince ?? Date.now()) : undefined;
			if (quietSince !== undefined && Date.now() - quietSince > GRACE_MS) {
				return setDraft({ at: 'failed', text: 'The agent finished without leaving a plan.', box: now.box });
			}
			setDraft({ at: 'running', box: now.box, line: detail.line, quietSince });
		} catch (e) {
			if (live.current) setDraft({ at: 'failed', text: why(e), box: now.box });
		}
	};
	const start = async () => {
		setDraft({ at: 'running', box: '', line: 'Starting a task to read the repos…' });
		try {
			const box = await planDraft(csrf, repos, prompt);
			if (live.current) setDraft({ at: 'running', box, line: 'Setting up…' });
		} catch (e) {
			if (live.current) setDraft({ at: 'failed', text: why(e) });
		}
	};
	const cancel = async (box: string) => {
		setDraft({ at: 'idle', note: 'Cancelled. The plan in the editor is unchanged.' });
		await boxClose(csrf, box).catch((e: unknown) => live.current && setDraft({ at: 'failed', text: why(e), box }));
	};
	const ready = repos.length > 0 && prompt.trim() !== '';
	if (running) {
		return (
			<div class="pd pd-running" role="status" aria-live="polite">
				<span class="pd-spin" aria-hidden="true" />
				<span class="grow">
					<strong>Drafting a plan.</strong> <span class="muted">{running.line}</span>
				</span>
				{running.box && (
					<>
						<a href={`#/box/${encodeURIComponent(running.box)}`} target="_blank" rel="noopener">
							Watch
						</a>
						<button type="button" class="quiet" onClick={() => void cancel(running.box)}>
							Cancel
						</button>
					</>
				)}
			</div>
		);
	}
	return (
		<div class="pd">
			<button type="button" disabled={!ready} title={ready ? 'Replaces the plan in the editor; nothing is saved until you save.' : 'Write the prompt and pick a repo first.'} onClick={() => void start()}>
				Generate plan
			</button>
			<span class="muted grow">An agent reads the repos and the prompt, then drafts the steps. It changes nothing.</span>
			{draft.at === 'idle' && draft.note && <p class="notice ok pd-note" role="status">{draft.note}</p>}
			{draft.at === 'failed' && (
				<p class="notice danger pd-note" role="alert">
					{draft.text}{' '}
					{draft.box && <a href={`#/box/${encodeURIComponent(draft.box)}`}>Open the task</a>}
				</p>
			)}
		</div>
	);
}
