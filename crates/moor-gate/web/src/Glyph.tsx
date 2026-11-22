import type { Progress } from './api';

/** The words and the glyph for each state; the controller derives the state, the page only names it. */
export const STATES: Record<Progress, { word: string; glyph: string; tone: string }> = {
	needs_you: { word: 'needs you', glyph: '●', tone: 'needs' },
	working: { word: 'working', glyph: '◐', tone: 'working' },
	setting_up: { word: 'setting up', glyph: '◌', tone: 'working' },
	queued: { word: 'queued', glyph: '○', tone: 'quiet' },
	done: { word: 'done', glyph: '✓', tone: 'done' },
	asleep: { word: 'asleep', glyph: '◒', tone: 'quiet' },
	failed: { word: 'failed', glyph: '✕', tone: 'failed' },
	closed: { word: 'closed', glyph: '–', tone: 'quiet' },
};

export function Glyph({ state, stale }: { state: Progress; stale: boolean }) {
	const s = STATES[state];
	return (
		<span class={`t-glyph t-${s.tone}${stale ? ' t-stale' : ''}`} role="img" aria-label={s.word} title={s.word}>
			{s.glyph}
		</span>
	);
}
