import type { BoxRow, Progress } from './api';

/** The words and the glyph for each state; the controller derives the state, the page only names it. */
export const STATES: Record<Progress, { word: string; glyph: string; tone: string }> = {
	needs_you: { word: 'needs you', glyph: '●', tone: 'needs' },
	working: { word: 'working', glyph: '◐', tone: 'working' },
	setting_up: { word: 'setting up', glyph: '◌', tone: 'working' },
	queued: { word: 'queued', glyph: '○', tone: 'quiet' },
	done: { word: 'done', glyph: '✓', tone: 'done' },
	ready: { word: 'ready', glyph: '○', tone: 'quiet' },
	asleep: { word: 'asleep', glyph: '◒', tone: 'quiet' },
	failed: { word: 'failed', glyph: '✗', tone: 'failed' },
	closed: { word: 'closed', glyph: '–', tone: 'quiet' },
};

/** How a finished run's outcome reads on a list row: a bad one takes "!", a plain one "·", a good one the tick. */
const OUTCOME_MARKS = {
	bad: { glyph: '!', tone: 'warn', word: 'needs a look' },
	plain: { glyph: '·', tone: 'done', word: 'nothing to do' },
	good: { glyph: '✓', tone: 'done', word: 'done' },
} as const;

/** The tone of the outcome a finished Box ended in, when its gates named one. */
export function outcomeTone(t: Pick<BoxRow, 'gate'>): keyof typeof OUTCOME_MARKS | null {
	const status = t.gate?.status;
	return status?.state === 'passed' ? (status.outcome?.tone ?? null) : null;
}

/** The mark a done row wears; the same for every other state as the state's own glyph. */
function markOf(state: Progress, outcome: keyof typeof OUTCOME_MARKS | null) {
	return state === 'done' && outcome ? OUTCOME_MARKS[outcome] : STATES[state];
}

/** Whether the row's now-text reads as a warning: it ended on a bad outcome. */
export const isWarn = (state: Progress, outcome: keyof typeof OUTCOME_MARKS | null) => state === 'done' && outcome === 'bad';

export function Glyph({ state, stale, outcome = null, settled }: { state: Progress; stale: boolean; outcome?: keyof typeof OUTCOME_MARKS | null; settled?: boolean }) {
	const s = settled ? { glyph: '○', tone: 'done', word: 'settled' } : markOf(state, outcome);
	return (
		<span class={`t-glyph t-${s.tone}${stale ? ' t-stale' : ''}`} role="img" aria-label={s.word} title={s.word}>
			{s.glyph}
		</span>
	);
}
