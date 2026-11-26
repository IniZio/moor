import type { ComponentChildren } from 'preact';
import { useState } from 'preact/hooks';

import { type BoxDetail, boxAnswer, failure, type Needs, toError } from './api';
import { HeaderActions, RUNNING } from './Ending';
import { FailedCard } from './Retry';
import { OutputList } from './PlaceCards';
import { PlaceStory } from './PlaceStory';
import { PrWord } from './PrHeader';
import { POLL_MS } from './Portal';
import { Composer } from './Steer';
import { TaskHead } from './TaskHead';
import './place.css';

/** How often the page of a task asked a moment ago looks again: the person is waiting on the answer. */
const FAST_MS = 1000;

/** How long after it was asked a task counts as young. */
const YOUNG_MS = 60_000;

/** How often to ask again for `detail` at `now`: fast while a question's task is young, the usual pace otherwise. */
export function askedPollMs(detail: Pick<BoxDetail, 'task_at'> | null, now: number): number {
	const asked = detail?.task_at ? Date.parse(detail.task_at) : Number.NaN;
	return now - asked < YOUNG_MS ? FAST_MS : POLL_MS;
}

/** What the agent's question shows: its words, its answers as small buttons with the first one filled, and a link to answer in the person's own words instead. */
export function AskView({ needs, busy, error, pick, own }: { needs: Needs; busy: boolean; error: string; pick: (n: number) => void; own: () => void }) {
	return (
		<section class="t-pl-ask" role="region" aria-label="Your input is needed">
			<p class="t-pl-aq">{needs.question}</p>
			{needs.detail && <pre class="t-quote">{needs.detail}</pre>}
			<div class="t-pl-aacts">
				{needs.picks.map((p, i) => (
					<button type="button" class={i === 0 ? 't-qbtn t-qbtn-primary' : 't-qbtn'} disabled={busy} onClick={() => pick(p.n)} key={p.n}>
						{p.label}
					</button>
				))}
				{needs.other != null && (
					<button type="button" class="t-qlnk" onClick={own}>
						or answer in your own words
					</button>
				)}
			</div>
			{error && (
				<p class="notice danger" role="alert">
					{error}
				</p>
			)}
		</section>
	);
}

/** The message box answers an open question in the person's own words. */
const toComposer = () => document.querySelector<HTMLTextAreaElement>('.t-place .t-say textarea')?.focus();

function PlaceAsk({ box, needs, csrf, onSent }: { box: string; needs: Needs; csrf: string; onSent: () => void }) {
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState('');
	const pick = async (n: number) => {
		if (busy) return;
		setBusy(true);
		setError('');
		try {
			await boxAnswer(csrf, box, needs.id, n);
		} catch (e) {
			setError(failure(toError(e)));
		} finally {
			setBusy(false);
			onSent();
		}
	};
	return <AskView needs={needs} busy={busy} error={error} pick={(n) => void pick(n)} own={toComposer} />;
}

/** A plain question the agent asks, which the conversation shows as its own; a permission, a gate or a plan keeps the page's card. */
const plainQuestion = (d: BoxDetail): Needs | null => (d.state === 'needs_you' && d.needs?.kind === 'question' && !d.needs.gate && !d.needs.plan && !d.needs.approve ? d.needs : null);

/** Keeps the conversation at its end as it grows while the person is there, as after they send; it stays put once they scroll up to read. */
function follow(el: HTMLElement | null) {
	if (!el) return;
	let atEnd = el.scrollHeight <= el.clientHeight + 8;
	el.addEventListener('scroll', () => (atEnd = el.scrollTop + el.clientHeight >= el.scrollHeight - 8));
	new MutationObserver(() => atEnd && el.scrollTo({ top: el.scrollHeight })).observe(el, { childList: true, subtree: true });
}

/** A question's task: the conversation, the agent's open question at its end, and a box to write more in, with `you` as the name over the person's messages. It has no work Box, so there is no branch, machine, changes, terminal or prompt to show, and Stop shows only while the agent works: it ends the agent and leaves the Box. */
export function AskedTask({ detail, error, now, csrf, reload, card = null, you = 'You' }: { detail: BoxDetail; error: string; now: number; csrf: string; reload: () => void; card?: ComponentChildren; you?: string }) {
	const work = detail.task_boxes.at(-1);
	const prState = detail.pr && work ? <PrWord box={work.name} number={detail.pr.number} /> : undefined;
	const asked = plainQuestion(detail);
	return (
		<div class="t-box t-place">
			<TaskHead t={detail} now={now} actions={RUNNING.includes(detail.state) ? <HeaderActions part="stop" detail={detail} csrf={csrf} onDone={reload} /> : null} />
			{error && (
				<p class="notice danger" role="alert">
					{error}
				</p>
			)}
			<FailedCard detail={detail} csrf={csrf} onDone={reload} />
			{detail.outputs.length > 0 && <OutputList outputs={detail.outputs} />}
			<section class="t-pl-conv" ref={follow}>
				<PlaceStory detail={detail} you={you} prState={prState} end={asked ? <PlaceAsk box={detail.box} needs={asked} csrf={csrf} onSent={reload} key={asked.id} /> : card} />
			</section>
			<Composer detail={detail} csrf={csrf} onSent={reload} plain />
		</div>
	);
}
