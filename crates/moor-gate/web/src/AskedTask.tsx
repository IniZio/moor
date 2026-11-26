import type { ComponentChildren } from 'preact';
import { useState } from 'preact/hooks';

import { type BoxDetail, boxAnswer, failure, type Needs, toError } from './api';
import { ChangesTab } from './Changes';
import { HeaderActions, RUNNING } from './Ending';
import { type Docs, GateStory } from './GateStory';
import { FailedCard } from './Retry';
import { PlaceStory } from './PlaceStory';
import { PrWord } from './PrHeader';
import { POLL_MS } from './Portal';
import { Composer } from './Steer';
import type { Tab } from './TaskPage';
import { TaskHead } from './TaskHead';
import { TerminalTab } from './Terminal';
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

/** Keeps the conversation at its end as it grows while the person is there, as after they send or when a picture loads; it stays put once they scroll up to read. */
export function follow(el: HTMLElement | null) {
	if (!el) return;
	let atEnd = el.scrollHeight <= el.clientHeight + 8;
	const toEnd = () => atEnd && el.scrollTo({ top: el.scrollHeight });
	const grown = new ResizeObserver(toEnd);
	const watch = () => {
		for (const child of el.children) grown.observe(child);
	};
	el.addEventListener('scroll', () => (atEnd = el.scrollTop + el.clientHeight >= el.scrollHeight - 8));
	new MutationObserver(() => {
		watch();
		toEnd();
	}).observe(el, { childList: true, subtree: true });
	watch();
}

/** A question's task, or a workflow run, as a conversation: the agent's open question at its end, and a box to write more in, with `you` as the name over the person's messages. Stop shows only while the agent works. A question's task has no Box of its own, so nothing more; a run's own Box keeps its terminal, changes and Close under the head's menu, its gates above the conversation, and Steer and Interrupt while the agent works. `tab` opens the terminal or the changes in place of the conversation. Another person's task (`owner`) is only read: no answer, composer, Stop or menu. */
export function AskedTask({ detail, error, now, csrf, reload, card = null, you = 'You', tab = 'timeline', docs = null, owner }: { detail: BoxDetail; error: string; now: number; csrf: string; reload: () => void; card?: ComponentChildren; you?: string; tab?: Tab; docs?: Docs | null; owner?: string }) {
	const run = !detail.task_at;
	const work = run ? undefined : detail.task_boxes.at(-1);
	const prBox = run ? detail.box : work?.name;
	const prState = detail.pr && prBox ? <PrWord box={prBox} number={detail.pr.number} /> : undefined;
	const asked = owner ? null : plainQuestion(detail);
	const running = RUNNING.includes(detail.state);
	const actions = owner ? <span class="t-readonly">{owner}'s task</span> : run ? <HeaderActions part={running ? undefined : 'more'} detail={detail} csrf={csrf} onDone={reload} changes /> : running ? <HeaderActions part="stop" detail={detail} csrf={csrf} onDone={reload} /> : null;
	const view = !run || owner ? null : tab === 'terminal' ? <TerminalTab box={detail.box} csrf={csrf} /> : tab === 'changes' ? <ChangesTab box={detail.box} seen={detail.last_event_at ?? detail.updated_at} /> : null;
	return (
		<div class="t-box t-place">
			<TaskHead t={detail} now={now} place asked={run ? detail.timeline[0]?.at : undefined} actions={actions} />
			{error && (
				<p class="notice danger" role="alert">
					{error}
				</p>
			)}
			{!owner && (!run || detail.retryable) && <FailedCard detail={detail} csrf={csrf} onDone={reload} />}
			{run && detail.gate?.gates.length ? <GateStory gate={detail.gate} docs={docs} card={null} wait={detail.state === 'needs_you' ? detail.needs?.gate : null} /> : null}
			{view ? (
				<section class="t-pl-view">
					<a class="t-pl-back" href={`#/box/${encodeURIComponent(detail.box)}`}>
						← Back to the conversation
					</a>
					{view}
				</section>
			) : (
				<section class="t-pl-conv" ref={follow}>
					<PlaceStory detail={detail} you={you} prState={prState} end={asked ? <PlaceAsk box={detail.box} needs={asked} csrf={csrf} onSent={reload} key={asked.id} /> : card} />
				</section>
			)}
			{owner ? <p class="t-readonly-note muted">You can read this task. Only {owner} can answer, steer or close it.</p> : <Composer detail={detail} csrf={csrf} onSent={reload} plain={!(run && detail.state === 'working')} />}
		</div>
	);
}
