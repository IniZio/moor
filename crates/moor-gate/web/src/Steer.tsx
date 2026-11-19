import { useEffect, useRef, useState } from 'preact/hooks';

import { type BoxDetail, type Needs, type Progress, failure, toError } from './api';
import { useDraft } from './session';
import { boxSteer, InterruptedOnly, type Over, type Sent } from './steerApi';
import './steer.css';

type Mode = 'steer' | 'interrupt';

/** The states in which the agent has no process to steer yet or any more, with what the box says then. */
const NOT_NOW: Partial<Record<Progress, string>> = {
	closed: 'This task is closed.',
	failed: 'This task failed. Start it again from the card above.',
	queued: 'The agent has not started yet. You can write to it once it has.',
	setting_up: 'The agent has not started yet. You can write to it once it has.',
};

/** The prompt entry a message goes in by while a card is open: the card's deny, or its own-words entry. */
function overOf(needs: Needs | null | undefined): Over | undefined {
	if (!needs) return undefined;
	const pick = needs.kind === 'permission' ? needs.picks.find((p) => p.role === 'deny')?.n : (needs.other ?? undefined);
	return pick ? { ask: needs.id, pick } : undefined;
}

/** What sending does, said before it does it. */
function hint(mode: Mode, state: Progress, needs: Needs | null | undefined, over: Over | undefined): string {
	if (over && needs?.kind === 'permission') return 'Sending denies the request above and passes your message on to the agent.';
	if (over) return 'Sending answers the question above in your own words.';
	if (state === 'asleep') return 'This Box is asleep. Sending wakes it, and the agent reads your message once it is up.';
	return mode === 'interrupt'
		? 'The agent stops its turn now and reads your message.'
		: 'The agent reads your message after its current tool call.';
}

/** A message that waits in the Box's inbox, shown as a You row until the timeline holds it. */
type Waiting = { text: string; at: number; why: string };

function queuedWhy(sent: Sent): string {
	if (sent.delivery === 'in_doubt') return 'The send may not have gone through. moor sends it once more when the agent is ready.';
	const cause = (sent.cause ?? 'busy').replace(/_/g, ' ');
	return cause === 'asleep' ? 'Waking the Box. The agent gets it once it is up.' : `Queued while the Box is ${cause}.`;
}

/** The message box at the bottom of a Box page: Steer or Interrupt, Enter to send, Shift+Enter for a new line. */
export function Composer({ detail, csrf, onSent }: { detail: BoxDetail; csrf: string; onSent: () => void }) {
	const [mode, setMode] = useState<Mode>('steer');
	const [text, setText] = useDraft(`steer:${detail.box}`);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState('');
	const [waiting, setWaiting] = useState<Waiting[]>([]);
	const area = useRef<HTMLTextAreaElement>(null);
	const asleep = detail.state === 'asleep';
	const over = detail.state === 'needs_you' ? overOf(detail.needs) : undefined;
	const refusal = NOT_NOW[detail.state];
	const how = asleep ? 'steer' : mode;

	useEffect(() => {
		const now = detail.timeline.filter((e) => e.class === 'you').map((e) => e.text);
		setWaiting((w) => w.filter((m) => !now.some((t) => t.includes(m.text)) && Date.now() - m.at < 10 * 60 * 1000));
	}, [detail.timeline]);

	const send = async () => {
		const said = text.trim();
		if (!said || busy || refusal) return;
		setBusy(true);
		setError('');
		try {
			const sent = await boxSteer(csrf, detail.box, said, how === 'interrupt', over);
			setText('');
			setMode('steer');
			setTimeout(() => window.scrollTo({ top: document.documentElement.scrollHeight, behavior: 'smooth' }), 600);
			if (sent.delivery !== 'delivered') setWaiting((w) => [...w, { text: said, at: Date.now(), why: queuedWhy(sent) }]);
		} catch (e) {
			if (e instanceof InterruptedOnly) onSent();
			setError(failure(toError(e)));
		} finally {
			setBusy(false);
			onSent();
			area.current?.focus();
		}
	};

	return (
		<div class="t-say" role="region" aria-label="Message the agent">
			{waiting.map((m) => (
				<div class="t-ev t-you t-queued" key={m.at}>
					<span class="t-who">You</span>
					<span class="t-when">queued</span>
					<span class="t-what">
						{m.text}
						<span class="t-queued-why">{m.why}</span>
					</span>
				</div>
			))}
			{refusal ? (
				<p class="muted t-say-off">{refusal}</p>
			) : (
				<form
					onSubmit={(e) => {
						e.preventDefault();
						void send();
					}}
				>
					<div class="t-say-row">
						<div class="t-seg" role="radiogroup" aria-label="How to send">
							{(['steer', 'interrupt'] as const).map((m) => (
								<label class="t-seg-opt" key={m} title={m === 'steer' ? 'After its current tool call' : 'Stop its turn now'}>
									<input type="radio" name="say-mode" value={m} checked={how === m} disabled={busy || (asleep && m === 'interrupt')} onChange={() => setMode(m)} />
									<span>{m === 'steer' ? 'Steer' : 'Interrupt'}</span>
								</label>
							))}
						</div>
						<textarea
							ref={area}
							rows={Math.min(6, Math.max(1, text.split('\n').length))}
							value={text}
							disabled={busy}
							placeholder={over ? 'Say what to do instead…' : how === 'interrupt' ? 'Interrupt the agent…' : 'Steer the agent…'}
							aria-label="Message to the agent"
							onInput={(e) => setText(e.currentTarget.value)}
							onKeyDown={(e) => {
								if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
									e.preventDefault();
									void send();
								}
							}}
						/>
						<button type="submit" class="btn primary" disabled={busy || !text.trim()}>
							{busy ? 'Sending…' : 'Send'}
						</button>
					</div>
					<p class="muted t-say-hint">{hint(how, detail.state, detail.needs, over)}</p>
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
