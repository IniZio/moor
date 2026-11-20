import type { RunStatus } from './api';

const ICONS: Record<RunStatus, { glyph: string; word: string }> = {
	queued: { glyph: '○', word: 'queued' },
	running: { glyph: '●', word: 'running' },
	waiting: { glyph: '◷', word: 'waiting on you' },
	success: { glyph: '✓', word: 'delivered' },
	failure: { glyph: '✗', word: 'failed' },
	stopped: { glyph: '■', word: 'declined' },
	skipped: { glyph: '–', word: 'skipped' },
};

export function when(ms: number | null | undefined): string {
	return ms == null ? 'none' : new Date(ms).toLocaleString();
}

export function duration(ms: number | null | undefined): string {
	if (ms == null) return '';
	const secs = Math.round(ms / 1000);
	if (secs < 60) return `${secs}s`;
	const mins = Math.floor(secs / 60);
	return mins < 60 ? `${mins}m ${secs % 60}s` : `${Math.floor(mins / 60)}h ${mins % 60}m`;
}

export function StatusIcon({ status }: { status: RunStatus }) {
	const icon = ICONS[status];
	return <span class={`status ${status}`} role="img" aria-label={icon.word} title={icon.word}>{icon.glyph}</span>;
}

export function Failed({ error, retry }: { error: string; retry: () => void }) {
	return (
		<>
			<p class="notice danger" role="alert">{error}</p>
			<button onClick={retry}>Try again</button>
		</>
	);
}

/** Marks a workflow that also runs on pull requests. */
export function PullTag() {
	return (
		<span class="pill ok" title="Runs on pull requests">
			PR
		</span>
	);
}

/** A run's pull request, `OWNER/NAME#NUMBER`, as a link to it on GitHub. */
export function PullLink({ pull }: { pull: string }) {
	const m = /^([\w.-]+\/[\w.-]+)#(\d+)$/.exec(pull);
	if (!m) return <span class="muted">{pull}</span>;
	return (
		<a class="run-pr" href={`https://github.com/${m[1]}/pull/${m[2]}`} target="_blank" rel="noopener noreferrer">
			{pull}
		</a>
	);
}

/** A workflow's repos in order: the agent starts in the first, and the rest are cloned with it. */
export function RepoSet({ repos }: { repos: string[] }) {
	return (
		<span class="repo-set">
			{repos.map((r, i) => (
				<span key={r} class={i === 0 ? 'pill ok' : 'pill'} title={i === 0 ? 'The agent starts here' : undefined}>
					{r}
				</span>
			))}
		</span>
	);
}
