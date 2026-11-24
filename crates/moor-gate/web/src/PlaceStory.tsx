import type { BoxDetail, Entry } from './api';
import { Markdown } from './Markdown';
import { clock, Line } from './Portal';
import { foldWords, routine, ToolLine } from './Transcript';

/** One question and what came of it: the person's message, the agent's last words as the answer, and everything else the agent did or said before them as its work. */
type Turn = { you?: Entry; answer?: Entry; work: Entry[]; ended?: Entry };

const failing = (e: Entry) => e.class === 'error' || !!e.tool?.failed;

/** The question's task cut at each message of the person; a turn's setup notes are dropped, its last words are the answer, and its calls, errors and earlier words are its work. When the task failed, the last failure nothing from the agent followed ends it. */
function turnsOf(entries: Entry[], failed: boolean): Turn[] {
	const turns: Turn[] = [];
	let turn: Turn = { work: [] };
	/** Takes the turn's last words out of its work, and returns how many work entries came before them. */
	const settle = (): number => {
		const at = turn.work.map((e) => e.class).lastIndexOf('message');
		if (at >= 0) turn.answer = turn.work.splice(at, 1)[0];
		return Math.max(at, 0);
	};
	const close = (ends = false) => {
		const settled = settle();
		if (ends) {
			const at = lastFailing(turn.work);
			if (at >= settled) turn.ended = turn.work.splice(at, 1)[0];
		}
		if (turn.you || turn.answer || turn.work.length || turn.ended) turns.push(turn);
		turn = { work: [] };
	};
	for (const e of entries) {
		if (e.class === 'you') {
			close();
			turn.you = e;
		} else if (e.class !== 'routine' && e.class !== 'hidden' && e.class !== 'turn') {
			turn.work.push(e);
		}
	}
	close(failed);
	return turns;
}

const lastFailing = (work: Entry[]) => work.map(failing).lastIndexOf(true);

const plural = (n: number, one: string, more: string) => `${n} ${n === 1 ? one : more}`;

/** What the fold of a turn's work says: the reads and searches the transcript names, then every other call as a command. */
function workWords(work: Entry[]): string {
	const calls = work.filter((e) => e.tool || e.class === 'step');
	const grouped = calls.filter(routine);
	const ran = calls.length - grouped.length;
	const said = [grouped.length ? foldWords(grouped) : '', ran ? `${grouped.length ? 'ran' : 'Ran'} ${plural(ran, 'command', 'commands')}` : ''].filter(Boolean).join(', ');
	return said || plural(work.length, 'note', 'notes');
}

function Work({ entry, gates }: { entry: Entry; gates: readonly string[] }) {
	if (entry.class === 'message')
		return (
			<div class="t-pl-note">
				<Markdown text={entry.text} />
			</div>
		);
	if (entry.tool) return <ToolLine e={entry} when={clock(entry.at)} />;
	if (entry.class === 'step') return <p class="t-pl-step t-mono">{entry.text}</p>;
	return <Line e={entry} gates={gates} />;
}

function Ended({ e }: { e: Entry }) {
	return (
		<div class="t-pl-ended" role="alert">
			<p class="t-pl-ended-h">{e.tool ? e.tool.title : 'Something went wrong'}</p>
			<pre>{e.tool?.output?.text ?? e.text}</pre>
		</div>
	);
}

/** A question's task as a conversation: the person's messages as bubbles under their name and time, the agent's last words as a line, one quiet fold of its work and earlier words under it, and only a failure that ended the task as an error. */
export function PlaceStory({ detail, you }: { detail: BoxDetail; you: string }) {
	const turns = turnsOf(detail.timeline, detail.state === 'failed');
	if (!turns.length) return <p class="t-empty">Nothing has happened yet.</p>;
	const gates = detail.gate?.gates ?? [];
	return (
		<div class="t-pl">
			{turns.flatMap((t, i) => [
				...(t.you
					? [
							<div class="t-pl-you" key={`y${i}`}>
								<span class="t-pl-meta">
									{you} · {clock(t.you.at)}
								</span>
								<span class="t-pl-bubble">{t.you.text === '(interrupt)' ? <em>Interrupted the agent's turn</em> : t.you.text}</span>
							</div>,
						]
					: []),
				...(t.answer
					? [
							<div class="t-pl-say" key={`s${i}`}>
								<span class="t-pl-mark">◆</span>
								<div class="t-pl-text">
									<Markdown text={t.answer.text} />
								</div>
							</div>,
						]
					: []),
				...(t.work.length
					? [
							<details class="t-pl-fold" key={`f${i}`}>
								<summary>
									<span class="t-chev">▸</span> {workWords(t.work)}
								</summary>
								<div class="t-pl-body">
									{t.work.map((e) => (
										<Work entry={e} gates={gates} key={e.key ?? e.id} />
									))}
								</div>
							</details>,
						]
					: []),
				...(t.ended ? [<Ended e={t.ended} key={`e${i}`} />] : []),
			])}
		</div>
	);
}
