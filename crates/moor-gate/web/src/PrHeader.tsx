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

/** Where the Box's pull request stands on GitHub, and a link to its next step. Other repos are listed below the first. The task head already shows the number `listed`, so this line does not repeat it. */
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
	const [first, ...rest] = pulls ?? [];
	const silent = !error && (!first || (!first.url && !first.unread && (first.state === 'none' || first.state === 'unpushed') && !named));
	if (silent) return null;
	if (!first) {
		return (
			<p class="t-prline pr-line">
				<span class="pr-label">PR</span>
				{named && named.number !== listed ? (
					<a class="t-mono" href={named.url} rel="noopener">
						#{named.number}
					</a>
				) : (
					<span class="muted">{error}</span>
				)}
			</p>
		);
	}
	return (
		<div class="pr-head">
			<p class="t-prline pr-line">
				<span class="pr-label">PR</span>
				{first.unread ? <span class="muted">GitHub could not be read: {first.unread}</span> : <State p={first} listed={listed} />}
				{(first.state === 'none' || first.state === 'unpushed') && named && (
					<a class="t-mono" href={named.url} rel="noopener" title="The agent named this PR, but none on GitHub is from this task's branch">
						{named.number === listed ? 'named by the agent' : `#${named.number} named by the agent`}
					</a>
				)}
				<span class="grow" />
				{first.url && (
					<a class={`btn${first.state === 'none' ? ' primary' : ''}${first.state === 'merged' ? ' pr-done' : ''}`} href={first.url} target="_blank" rel="noopener">
						{step(first)} ↗
					</a>
				)}
			</p>
			{rest.length > 0 && (
				<ul class="pr-others">
					{rest.map((p) => (
						<li key={p.repo}>
							<span class="t-mono muted">{p.repo}</span>
							{p.unread ? <span class="muted">GitHub could not be read</span> : <State p={p} listed={null} />}
							{p.url && (
								<a href={p.url} target="_blank" rel="noopener">
									{step(p)} ↗
								</a>
							)}
						</li>
					))}
				</ul>
			)}
		</div>
	);
}
