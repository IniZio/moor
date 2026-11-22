import type { ComponentChildren } from 'preact';
import { useEffect } from 'preact/hooks';

import type { BoxRow } from './api';
import { gateLine, peekable } from './gate';
import { Glyph, STATES } from './Glyph';
import { type Grouping, homeFilter, peekOf, withQuery } from './hashQuery';

/** A done Box idle this long moves to the Settled shelf. */
const SETTLE_MS = 7 * 24 * 3600 * 1000;

/** Which list a Box sits in. */
type Group = 'needs' | 'working' | 'done' | 'settled';

function groupOf(t: BoxRow, now: number): Group {
	switch (t.state) {
		case 'needs_you':
			return 'needs';
		case 'working':
		case 'setting_up':
		case 'queued':
			return 'working';
		case 'closed':
			return 'settled';
		default:
			return t.state === 'done' && now - Date.parse(t.last_event_at ?? t.updated_at) > SETTLE_MS ? 'settled' : 'done';
	}
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

const two = (n: number) => String(n).padStart(2, '0');

/** An exact time in the viewer's zone: the clock today, the day and clock this year, the year as well before that. */
export function exactTime(iso: string | null | undefined, now: number): string {
	const at = new Date(iso ?? '');
	if (Number.isNaN(at.getTime())) return '';
	const today = new Date(now);
	const clock = `${two(at.getHours())}:${two(at.getMinutes())}`;
	if (at.getFullYear() === today.getFullYear() && at.getMonth() === today.getMonth() && at.getDate() === today.getDate()) return clock;
	const day = `${at.getDate()} ${MONTHS[at.getMonth()]}`;
	return at.getFullYear() === today.getFullYear() ? `${day} ${clock}` : `${day} ${at.getFullYear()} ${clock}`;
}

/** How many colours a repo's dot can take; one CSS class each, since the page's CSP forbids inline styles. */
const DOT_COLOURS = 6;

/** The same repo always gets the same colour. */
function dotOf(repo: string): number {
	let h = 0;
	for (const c of repo) h = (h * 31 + c.charCodeAt(0)) >>> 0;
	return h % DOT_COLOURS;
}

/** The repos a Box works on, the one it starts in first. */
export function reposOf(t: Pick<BoxRow, 'repo' | 'other_repos'>): string[] {
	return [t.repo, ...t.other_repos].filter((r): r is string => !!r);
}

export function RepoName({ repo }: { repo: string }) {
	return (
		<>
			<Dot repo={repo} /> <span class="t-mono">{repo}</span>
		</>
	);
}

function Dot({ repo }: { repo: string }) {
	return <span class={`t-rdot t-rc-${dotOf(repo)}`} aria-hidden="true" />;
}

/** The first repo with a dot and "+N" for the rest; under a repo filter, only the repos beside the filtered one. */
export function RepoCell({ repos, only }: { repos: string[]; only: string }) {
	const title = repos.length > 1 ? repos.join(', ') : undefined;
	if (only) {
		const others = repos.filter((r) => r !== only);
		return (
			<span class="t-rrepo" title={title}>
				{others.length > 0 && <span class="t-mono t-rmore">+ {others.join(' + ')}</span>}
			</span>
		);
	}
	return (
		<span class="t-rrepo" title={title}>
			{repos[0] && <Dot repo={repos[0]} />}
			<span class="t-mono t-rfirst">{repos[0] ?? ''}</span>
			{repos.length > 1 && <span class="t-mono t-rmore">+{repos.length - 1}</span>}
		</span>
	);
}

function Row({ t, now, only, quiet, peeking }: { t: BoxRow; now: number; only: string; quiet?: boolean; peeking?: boolean }) {
	const s = STATES[t.state];
	const gated = t.state === 'needs_you' || t.failed ? gateLine(t.gate) : null;
	const askable = peekable(t);
	return (
		<a
			class={`t-row t-${s.tone}${quiet ? ' t-settled-row' : ''}${peeking ? ' t-peeking' : ''}`}
			href={`#/box/${encodeURIComponent(t.box)}`}
			data-row
			data-box={t.box}
			data-peek={askable ? t.box : undefined}
		>
			<Glyph state={t.state} stale={t.stale} />
			<RepoCell repos={reposOf(t)} only={only} />
			<span class="t-pr" title={t.pr ? `${t.pr.repo}#${t.pr.number}` : undefined}>
				{t.pr ? `PR #${t.pr.number}` : ''}
			</span>
			<span class="t-title">{t.title}</span>
			<span class={`t-now${t.state === 'needs_you' ? ' t-ask' : ''}`}>
				{t.stale && <span class="t-warn">⚠ stale </span>}
				{t.failed && !gated && <span class="t-failword">failed </span>}
				{gated ?? t.line}
			</span>
			<time class="t-age" dateTime={t.last_event_at ?? t.updated_at}>
				{exactTime(t.last_event_at ?? t.updated_at, now)}
			</time>
			{askable && (
				<span class="t-peekbtn" role="button" tabIndex={-1} data-peekbtn={t.box} title="Answer here (Space)">
					peek
				</span>
			)}
		</a>
	);
}

function Section({ heading, tone, rows, now, only, peeking, empty }: { heading: ComponentChildren; tone: string; rows: BoxRow[]; now: number; only: string; peeking: string; empty?: string }) {
	return (
		<section class={`t-grp t-grp-${tone}`}>
			<h2>
				{heading} · {rows.length}
			</h2>
			{rows.length ? rows.map((t) => <Row t={t} now={now} only={only} peeking={t.box === peeking} key={t.box} />) : <p class="t-empty">{empty}</p>}
		</section>
	);
}

function Choice({ href, on, children }: { href: string; on: boolean; children: ComponentChildren }) {
	return (
		<a href={href} aria-current={on ? 'true' : undefined}>
			{children}
		</a>
	);
}

function Filters({ hash, repos, repo, grp }: { hash: string; repos: string[]; repo: string; grp: Grouping }) {
	return (
		<div class="t-filters">
			<details class="t-rmenu" key={repo}>
				<summary>{repo || 'All repos'} ▾</summary>
				<div class="t-rmenu-pop">
					<Choice href={withQuery(hash, { repo: '' })} on={!repo}>
						All repos
					</Choice>
					{repos.map((r) => (
						<Choice href={withQuery(hash, { repo: r })} on={r === repo} key={r}>
							{r}
						</Choice>
					))}
				</div>
			</details>
			<span>
				Group by{' '}
				<Choice href={withQuery(hash, { grp: '' })} on={grp === 'state'}>
					state
				</Choice>{' '}
				·{' '}
				<Choice href={withQuery(hash, { grp: 'repo' })} on={grp === 'repo'}>
					repo
				</Choice>
			</span>
		</div>
	);
}

/** Needs you first, then working, then the rest. */
const RANK: Record<Group, number> = { needs: 0, working: 1, done: 2, settled: 3 };

/** The Tasks home without its hooks, so a test can read it: `hash` holds the filter, the rows are the person's Boxes. */
export function HomeListView({ rows, now, hash }: { rows: BoxRow[]; now: number; hash: string }) {
	const known = [...new Set(rows.flatMap(reposOf))].sort();
	const filter = homeFilter(hash);
	const peeking = peekOf(hash);
	const bare = withQuery(hash, { peek: '' });
	const repo = known.includes(filter.repo) ? filter.repo : '';
	const kept = rows.filter((t) => !repo || reposOf(t).includes(repo));
	const by = (g: Group) => kept.filter((t) => groupOf(t, now) === g);
	const settled = by('settled');
	const open = kept.filter((t) => groupOf(t, now) !== 'settled');
	const stacks = (repo ? [repo] : [...known, '']).map((k) => ({
		name: k,
		rows: open.filter((t) => (repo || reposOf(t)[0] || '') === k).sort((a, b) => RANK[groupOf(a, now)] - RANK[groupOf(b, now)]),
	}));
	return (
		<div class="t-home">
			<p class="t-hint muted">
				<kbd>j</kbd> <kbd>k</kbd> move · <kbd>enter</kbd> open · <kbd>space</kbd> peek
			</p>
			<Filters hash={bare} repos={known} repo={repo} grp={filter.grp} />
			{filter.grp === 'repo' ? (
				stacks.map((s) =>
					s.rows.length ? <Section key={s.name} tone="repo" heading={s.name ? <RepoName repo={s.name} /> : 'No repo'} rows={s.rows} now={now} only={repo} peeking={peeking} /> : null,
				)
			) : (
				<>
					{repo && (
						<h2 class="t-repohead">
							<RepoName repo={repo} />
						</h2>
					)}
					<Section heading="Needs you" tone="needs" rows={by('needs')} now={now} only={repo} peeking={peeking} empty="Nothing needs you." />
					<Section heading="Working" tone="working" rows={by('working')} now={now} only={repo} peeking={peeking} empty="Nothing is running." />
					<Section heading="Done" tone="done" rows={by('done')} now={now} only={repo} peeking={peeking} empty="None." />
				</>
			)}
			<details class="t-settled">
				<summary>
					<span class="t-chev">▸</span> Settled · {settled.length} <span class="muted">tasks closed, or done and quiet for 7 days</span>
				</summary>
				{settled.map((t) => (
					<Row t={t} now={now} only={repo} key={t.box} quiet peeking={t.box === peeking} />
				))}
			</details>
			{!rows.length && <p class="t-empty">No tasks yet. Start one with New task, or from Claude Code with the moor tools.</p>}
		</div>
	);
}

/** Closes an open repo menu on Escape or a click outside it; a details element does neither itself. */
export function useMenuDismiss() {
	useEffect(() => {
		const shut = (keep: EventTarget | null) => {
			for (const d of document.querySelectorAll<HTMLDetailsElement>('details.t-rmenu[open]')) {
				if (!(keep instanceof Node && d.contains(keep))) d.open = false;
			}
		};
		const onClick = (e: MouseEvent) => shut(e.target);
		const onKey = (e: KeyboardEvent) => e.key === 'Escape' && shut(null);
		addEventListener('click', onClick);
		addEventListener('keydown', onKey);
		return () => {
			removeEventListener('click', onClick);
			removeEventListener('keydown', onKey);
		};
	}, []);
}
