import { useRef, useState } from 'preact/hooks';

import { type BoxDetail, failure } from './api';
import { boxClose, boxStop, Refused, type Unsaved } from './steerApi';
import './steer.css';

/** The states in which the agent runs. */
export const RUNNING = ['working', 'needs_you'];

/** The states of a Box task in which Stop has something to end: its agent may idle after it is done. */
const STOPPABLE = [...RUNNING, 'done'];

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

export function unsavedWords({ ahead, dirty }: Unsaved): string {
	const held = [dirty > 0 && count(dirty, 'uncommitted file', 'uncommitted files'), ahead > 0 && count(ahead, 'unpushed commit', 'unpushed commits')].filter(Boolean);
	return `This task has ${held.join(' and ')}. Push them from the terminal, or discard them and close.`;
}

function whyNot(e: Error): string {
	return e instanceof Refused && e.unsaved && (e.unsaved.ahead > 0 || e.unsaved.dirty > 0) ? unsavedWords(e.unsaved) : failure(e);
}

/** Stop beside the state (`part="stop"`), and the terminal and Close under a menu with a confirm (`part="more"`); Close removes the Box. Without a part, both. */
export function HeaderActions({ detail, csrf, onDone, part }: { detail: BoxDetail; csrf: string; onDone: () => void; part?: 'stop' | 'more' }) {
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState('');
	const [refused, setRefused] = useState('');
	const dialog = useRef<HTMLDialogElement>(null);
	const menu = useRef<HTMLDivElement>(null);
	const opener = useRef<HTMLButtonElement>(null);
	const closed = detail.state === 'closed';

	const run = async (go: () => Promise<void>, onError: (e: Error) => void) => {
		if (busy) return;
		setBusy(true);
		setError('');
		try {
			await go();
			onDone();
		} catch (e) {
			onError(e as Error);
		} finally {
			setBusy(false);
		}
	};
	const close = (force: boolean) =>
		run(
			async () => {
				await boxClose(csrf, detail.box, force);
				dialog.current?.close();
			},
			(e) => setRefused(whyNot(e)),
		);

	return (
		<div class="t-acts">
			{part !== 'more' && STOPPABLE.includes(detail.state) && (
				<button type="button" class="btn t-stop" disabled={busy} onClick={() => void run(() => boxStop(csrf, detail.box), (e) => setError(whyNot(e)))}>
					{busy ? 'Stopping…' : 'Stop'}
				</button>
			)}
			{part !== 'stop' && !closed && (
				<>
					<button type="button" class="btn ghost icon" ref={opener} popovertarget="t-more" aria-label="More actions" aria-haspopup="menu" disabled={busy}>
						⋯
					</button>
					<div
						id="t-more"
						ref={menu}
						class="pop t-more"
						popover="auto"
						onToggle={(e) => {
							const at = opener.current?.getBoundingClientRect();
							if (at && e.newState === 'open' && menu.current) {
								menu.current.style.top = `${at.bottom + 4}px`;
								menu.current.style.right = `${document.documentElement.clientWidth - at.right}px`;
							}
						}}
					>
						<a class="t-more-item" href={`#/box/${encodeURIComponent(detail.box)}/terminal`} onClick={() => menu.current?.hidePopover()}>
							Open the terminal
						</a>
						<button
							type="button"
							class="t-more-item t-more-danger"
							onClick={() => {
								menu.current?.hidePopover();
								setRefused('');
								dialog.current?.showModal();
							}}
						>
							Close task…
						</button>
					</div>
				</>
			)}
			{error && (
				<p class="notice danger t-act-err" role="alert">
					{error}
				</p>
			)}
			{part !== 'stop' && (
				<dialog ref={dialog} class="t-confirm" aria-labelledby="t-confirm-h" onClose={() => setRefused('')}>
					<h2 id="t-confirm-h">Close this task?</h2>
					<p class="muted">The Box is removed. Pushed commits and evidence stay.</p>
					{refused && (
						<p class="notice danger" role="alert">
							{refused}
						</p>
					)}
					<div class="actions">
						<button type="button" class="btn t-close-go" disabled={busy} onClick={() => void close(Boolean(refused))}>
							{busy ? 'Closing…' : refused ? 'Discard and close' : 'Close task'}
						</button>
						<button type="button" class="btn ghost" disabled={busy} onClick={() => dialog.current?.close()}>
							Keep it
						</button>
					</div>
				</dialog>
			)}
		</div>
	);
}
