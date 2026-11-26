import type { RunStatus } from './api';
import type { Verdict } from './gate';
import { rowTime } from './runGroups';

/** Each run state as its mark shows it: the glyph, the look the mark and its words take, and the word a reader hears. */
const MARK: Record<RunStatus, { glyph: string; look: string; word: string }> = {
	queued: { glyph: '○', look: 'settled', word: 'queued' },
	running: { glyph: '◐', look: 'working', word: 'running' },
	waiting: { glyph: '●', look: 'needs', word: 'waiting on you' },
	success: { glyph: '✓', look: 'done', word: 'delivered' },
	failure: { glyph: '✗', look: 'failed', word: 'failed' },
	stopped: { glyph: '■', look: 'settled', word: 'declined' },
	skipped: { glyph: '–', look: 'settled', word: 'skipped' },
};

/** A delivered run whose outcome reads bad is marked as a warning, never as a failure. */
export function markOf(status: RunStatus, verdict?: Verdict | null) {
	const m = MARK[status];
	if (status !== 'success' || !verdict) return m;
	const word = `${m.word}: ${verdict.name}`;
	return verdict.tone === 'bad' ? { glyph: '!', look: 'warn', word } : { ...m, word };
}

export function when(ms: number | null | undefined): string {
	return ms == null ? 'none' : rowTime(ms);
}

export function duration(ms: number | null | undefined): string {
	if (ms == null) return '';
	const secs = Math.round(ms / 1000);
	if (secs < 60) return `${secs}s`;
	const mins = Math.floor(secs / 60);
	return mins < 60 ? `${mins}m ${secs % 60}s` : `${Math.floor(mins / 60)}h ${mins % 60}m`;
}

export function StatusIcon({ status, verdict }: { status: RunStatus; verdict?: Verdict | null }) {
	const m = markOf(status, verdict);
	return (
		<span class={`smk smk-${m.look}`} role="img" aria-label={m.word} title={m.word}>
			{m.glyph}
		</span>
	);
}

export function Failed({ error, retry }: { error: string; retry: () => void }) {
	return (
		<>
			<p class="notice danger" role="alert">
				{error}
			</p>
			<button onClick={retry}>Try again</button>
		</>
	);
}
