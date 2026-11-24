import type { ComponentChildren } from 'preact';

import type { BoxDetail } from './api';
import { HeaderActions, RUNNING } from './Ending';
import { FailedCard } from './Retry';
import { PlaceStory } from './PlaceStory';
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

/** A question's task: the conversation, and a box to write more in, with `you` as the name over the person's messages. It has no work Box, so there is no branch, machine, changes, terminal or prompt to show, and Stop shows only while the agent works: it ends the agent and leaves the Box. */
export function AskedTask({ detail, error, now, csrf, reload, card = null, you = 'You' }: { detail: BoxDetail; error: string; now: number; csrf: string; reload: () => void; card?: ComponentChildren; you?: string }) {
	return (
		<div class="t-box t-place">
			<TaskHead t={detail} now={now} actions={RUNNING.includes(detail.state) ? <HeaderActions part="stop" detail={detail} csrf={csrf} onDone={reload} /> : null} />
			{error && (
				<p class="notice danger" role="alert">
					{error}
				</p>
			)}
			{card}
			<FailedCard detail={detail} csrf={csrf} onDone={reload} />
			<section>
				<PlaceStory detail={detail} you={you} />
			</section>
			<Composer detail={detail} csrf={csrf} onSent={reload} plain />
		</div>
	);
}
