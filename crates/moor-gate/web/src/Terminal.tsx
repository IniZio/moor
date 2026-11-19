import { useEffect, useRef, useState } from 'preact/hooks';

import type { Status } from './terminal';

const WORDS: Record<Status['kind'], string> = { opening: 'Connecting…', live: 'Live', ended: 'Ended' };

/** The Box's own Claude session, live: what you type goes straight to the agent. */
export function TerminalTab({ box, csrf }: { box: string; csrf: string }) {
	const host = useRef<HTMLDivElement>(null);
	const [status, setStatus] = useState<Status>({ kind: 'opening' });
	const [round, setRound] = useState(0);
	useEffect(() => {
		const el = host.current;
		if (!el) return;
		let gone = false;
		let close = () => {};
		setStatus({ kind: 'opening' });
		import('./terminal')
			.then(({ attach }) => attach(el, box, csrf, (s) => gone || setStatus(s)))
			.then(
				(closer) => (gone ? closer() : (close = closer)),
				(e: Error) => gone || setStatus({ kind: 'ended', why: e.message }),
			);
		return () => {
			gone = true;
			close();
			el.replaceChildren();
		};
	}, [box, csrf, round]);
	return (
		<div class="t-term">
			<div class="t-term-bar">
				<span class={`t-term-state t-term-${status.kind}`}>{WORDS[status.kind]}</span>
				<span class="muted">{status.kind === 'ended' ? status.why : 'Typing here goes straight to the agent. Closing this tab leaves the task running.'}</span>
				<span class="grow" />
				{status.kind === 'ended' && (
					<button type="button" class="btn" onClick={() => setRound((n) => n + 1)}>
						Reconnect
					</button>
				)}
			</div>
			<div class="t-term-screen" ref={host} />
		</div>
	);
}
