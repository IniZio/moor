import { useEffect, useState } from 'preact/hooks';
import { boxPulls, type BoxDetail, type RepoPull } from './api';

/** GitHub is read through moor's cache, so a slower poll than the page's own is enough. */
const PR_POLL_MS = 15000;

const STATES: Record<RepoPull['state'], { word: string; glyph: string; tone: string }> = {
	unpushed: { word: 'not pushed yet', glyph: '○', tone: 'none' },
	none: { word: 'none yet', glyph: '○', tone: 'none' },
	draft: { word: 'draft', glyph: '◌', tone: 'draft' },
	open: { word: 'open', glyph: '●', tone: 'open' },
	changes_requested: { word: 'changes requested', glyph: '!', tone: 'changes' },
	approved: { word: 'approved', glyph: '✓', tone: 'approved' },
	merged: { word: 'merged', glyph: '⬢', tone: 'merged' },
	closed: { word: 'closed', glyph: '–', tone: 'closed' },
};

const CHECKS: Record<NonNullable<RepoPull['checks']>, { word: string; glyph: string }> = {
	passing: { word: 'checks passing', glyph: '✓' },
	failing: { word: 'checks failing', glyph: '✕' },
	pending: { word: 'checks running', glyph: '◐' },
};

/** The next step on GitHub, which is always a link: moor never merges. None until the branch is pushed. */
function step(p: RepoPull): string | null {
	if (p.state === 'unpushed') return null;
	if (p.state === 'none') return 'Open PR';
	if (p.state === 'merged') return 'Merged';
	return 'View PR';
}

function State({ p, listed }: { p: RepoPull; listed: number | null }) {
	const s = STATES[p.state];
	const checks = p.checks ? CHECKS[p.checks] : null;
	return (
		<span class={`pr-state pr-${s.tone}`}>
			{p.number !== null && p.number !== listed && <span class="t-mono">#{p.number}</span>}
			<span class="pr-word">
				<span aria-hidden="true">{s.glyph}</span> {s.word}
			</span>
			{checks && (
				<span class={`pr-checks pr-${p.checks}`}>
					<span aria-hidden="true">{checks.glyph}</span> {checks.word}
				</span>
			)}
		</span>
	);
}

/** Where the Box's pull request stands on GitHub, inline in the task head after its PR link, and a link to its next step unless that link is the head's own. Other repos follow the first. The task head already shows the number `listed`, so this does not repeat it. */
export function PrInline({ pulls, error, named, listed }: { pulls: RepoPull[] | null; error: string; named: BoxDetail['pr']; listed: number | null }) {
	const [first, ...rest] = pulls ?? [];
	const silent = !error && (!first || (!first.url && !first.unread && (first.state === 'none' || first.state === 'unpushed') && !named));
	if (silent) return null;
	if (!first) {
		if (named && named.number !== listed) {
			return (
				<a class="t-mono" href={named.url} rel="noopener">
					#{named.number}
				</a>
			);
		}
		return error ? <span class="pr-inline muted">{error}</span> : null;
	}
	const own = named !== null && first.number === named.number && first.state !== 'none' && first.state !== 'unpushed';
	return (
		<span class="pr-inline">
			{first.unread ? <span class="muted">GitHub could not be read: {first.unread}</span> : <State p={first} listed={listed} />}
			{(first.state === 'none' || first.state === 'unpushed') && named && (
				<a class="t-mono" href={named.url} rel="noopener" title="The agent named this PR, but none on GitHub is from this task's branch">
					{named.number === listed ? 'named by the agent' : `#${named.number} named by the agent`}
				</a>
			)}
			{first.url && !own && (
				<a class={`pr-step${first.state === 'none' ? ' pr-go' : ''}${first.state === 'merged' ? ' pr-done' : ''}`} href={first.url} target="_blank" rel="noopener">
					{step(first)} ↗
				</a>
			)}
			{rest.map((p) => (
				<span class="pr-other" key={p.repo}>
					<span class="t-mono muted">{p.repo}</span>
					{p.unread ? <span class="muted">GitHub could not be read</span> : <State p={p} listed={null} />}
					{p.url && (
						<a class={`pr-step${p.state === 'none' ? ' pr-go' : ''}`} href={p.url} target="_blank" rel="noopener">
							{step(p)} ↗
						</a>
					)}
				</span>
			))}
		</span>
	);
}

/** `PrInline` for Box `box`, read through moor's cache of GitHub on a slow poll. */
export function PrHeader({ box, named, listed }: { box: string; named: BoxDetail['pr']; listed: number | null }) {
	const [pulls, setPulls] = useState<RepoPull[] | null>(null);
	const [error, setError] = useState('');
	useEffect(() => {
		let live = true;
		const run = () =>
			boxPulls(box).then(
				(next) => live && (setPulls(next), setError('')),
				(e: Error) => live && setError(e.message),
			);
		void run();
		const timer = setInterval(() => !document.hidden && void run(), PR_POLL_MS);
		return () => {
			live = false;
			clearInterval(timer);
		};
	}, [box]);
	return <PrInline pulls={pulls} error={error} named={named} listed={listed} />;
}
