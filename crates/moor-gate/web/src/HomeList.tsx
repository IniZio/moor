import type { ComponentChildren } from 'preact';
import { useEffect } from 'preact/hooks';

import type { BoxRow } from './api';
import { gateLine, peekable } from './gate';
import { Glyph, isWarn, outcomeTone, STATES } from './Glyph';
import { type Grouping, homeFilter, peekOf, type Whose, whoseHome, withQuery } from './hashQuery';

/** A done Box idle this long moves to the Settled shelf. */
const SETTLE_MS = 7 * 24 * 3600 * 1000;

/** Which list a Box sits in. */
type Group = 'needs' | 'working' | 'done' | 'settled';

/** Another person's task that waits on them is still working, as far as the reader goes. */
function groupOf(t: BoxRow, now: number): Group {
	switch (t.state) {
		case 'needs_you':
			return t.owner ? 'working' : 'needs';
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

/** How many colours a repo's dot can take (the mock's palette); one CSS class each, since the page's CSP forbids inline styles. */
const DOT_COLOURS = 5;

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
		<span class="t-rname">
			<Dot repo={repo} /> <span class="t-mono">{repo}</span>
		</span>
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

/** The tracker key in an issue as the person wrote it, such as WEB-142 in a link or a bare key. */
function issueKey(issue: string): string | null {
	return issue.match(/(?:^|[\s/])([A-Z]{2,6}-\d{1,5})(?:$|[\s/?#])/)?.[1] ?? null;
}

/** Where a task came from: its issue key, or the workflow and pull request of the run that made it; else its pull request, unless its line already names it. */
export function Source({ t }: { t: Pick<BoxRow, 'issue' | 'workflow' | 'pr' | 'line'> }) {
	if (t.workflow) {
		const number = t.workflow.pull?.split('#').pop() ?? t.pr?.number;
		return <span class="t-rkey">{number ? `${t.workflow.name} · PR #${number}` : t.workflow.name}</span>;
	}
	const key = t.issue ? (issueKey(t.issue) ?? t.issue) : '';
	if (key) return <span class="t-rkey t-rkey-issue">{key}</span>;
	const pr = t.pr ? `PR #${t.pr.number}` : '';
	return <span class="t-rkey">{pr && !t.line.includes(pr) ? pr : ''}</span>;
}

/** What a row adds before its title: the tracker key, else the pull request number unless the line already names it. */
function rowTag(t: Pick<BoxRow, 'issue' | 'workflow' | 'pr' | 'line'>): string {
	if (t.issue) return issueKey(t.issue) ?? t.issue;
	const number = t.workflow?.pull?.split('#').pop() ?? t.pr?.number;
	const pr = number ? `PR #${number}` : '';
	return pr && !t.line.includes(pr) ? pr : '';
}

function Row({ t, now, only, everyone, settled, peeking }: { t: BoxRow; now: number; only: string; everyone: boolean; settled?: boolean; peeking?: boolean }) {
	const s = STATES[t.state];
	const gated = t.state === 'needs_you' || t.state === 'working' || t.failed ? gateLine(t.gate) : null;
	const askable = !t.owner && peekable(t);
	const outcome = outcomeTone(t);
	const tag = rowTag(t);
	const tone = t.state === 'needs_you' ? ' t-n-needs' : t.state === 'working' || t.state === 'setting_up' ? ' t-n-working' : isWarn(t.state, outcome) ? ' t-n-warn' : '';
	return (
		<a
			class={`t-row t-${s.tone}${everyone ? ' t-everyone' : ''}${peeking ? ' t-peeking' : ''}`}
			href={`#/box/${encodeURIComponent(t.box)}`}
			data-row
			data-box={t.box}
			data-peek={askable ? t.box : undefined}
		>
			<span class="t-rm">
				<Glyph state={t.state} stale={t.stale} outcome={outcome} settled={settled} />
			</span>
			<RepoCell repos={reposOf(t)} only={only} />
			{everyone && <span class="t-rown">{t.owner ?? 'you'}</span>}
			<span class="t-title">
				{tag && <span class={t.issue ? 't-rkey t-rkey-issue' : 't-rkey'}>{tag}</span>}
				{t.title}
			</span>
			<span class={`t-now${tone}`}>
				{t.stale && <span class="t-warn">⚠ stale </span>}
				{t.failed && !gated && <span class="t-failword">failed </span>}
				{gated ?? t.line}
			</span>
			<time class="t-age" dateTime={t.last_event_at ?? t.updated_at}>
				{exactTime(t.last_event_at ?? t.updated_at, now)}
			</time>
			{askable && (
				<span class="t-peekbtn" role="button" tabIndex={-1} data-peekbtn={t.box} title="Answer here (Space)">
					{t.needs.gate.kind === 'approve' ? 'Review' : 'Look'}
				</span>
			)}
		</a>
	);
}

type Shown = { now: number; only: string; everyone: boolean; peeking: string };

function Section({ heading, tone, rows, empty, ...shown }: { heading: ComponentChildren; tone: string; rows: BoxRow[]; empty?: string } & Shown) {
	return (
		<section class={`t-grp t-grp-${tone}`}>
			<h2>
				{heading}
				<span class="t-cnt">
					<span class="t-sep"> · </span>
					{rows.length}
				</span>
			</h2>
			{rows.length ? (
				<div class="t-rows">
					{rows.map((t) => (
						<Row t={t} now={shown.now} only={shown.only} everyone={shown.everyone} peeking={t.box === shown.peeking} key={t.box} />
					))}
				</div>
			) : (
				<p class="t-empty">{empty}</p>
			)}
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

function Filters({ hash, repos, repo, grp, who }: { hash: string; repos: string[]; repo: string; grp: Grouping; who: Whose }) {
	return (
		<div class="t-filters">
			<span>
				<Choice href={withQuery(hash, { who: '' })} on={who === 'mine'}>
					Mine
				</Choice>{' '}
				·{' '}
				<Choice href={withQuery(hash, { who: 'everyone' })} on={who === 'everyone'}>
					Everyone
				</Choice>
			</span>
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

/** The repos any of the rows work on, the one with the newest activity first. */
function repoOrder(rows: BoxRow[]): string[] {
	const latest = new Map<string, number>();
	for (const t of rows) for (const r of reposOf(t)) latest.set(r, Math.max(latest.get(r) ?? 0, at(t)));
	return [...latest.keys()].sort((a, b) => (latest.get(b) ?? 0) - (latest.get(a) ?? 0) || a.localeCompare(b));
}

/** Newest first. */
const at = (t: BoxRow) => Date.parse(t.last_event_at ?? t.updated_at) || 0;
const newest = (a: BoxRow, b: BoxRow) => at(b) - at(a);

/** The Tasks home without its hooks, so a test can read it: `hash` holds the filter, the rows are the person's Boxes. */
export function HomeListView({ rows, now, hash }: { rows: BoxRow[]; now: number; hash: string }) {
	const known = repoOrder(rows);
	const filter = homeFilter(hash);
	const peeking = peekOf(hash);
	const bare = withQuery(hash, { peek: '' });
	const repo = known.includes(filter.repo) ? filter.repo : '';
	const kept = rows.filter((t) => !repo || reposOf(t).includes(repo));
	const by = (g: Group) => kept.filter((t) => groupOf(t, now) === g).sort(newest);
	const everyone = whoseHome(hash) === 'everyone';
	const settled = by('settled');
	const open = kept.filter((t) => groupOf(t, now) !== 'settled');
	const stacks = (repo ? [repo] : [...known, '']).map((k) => ({
		name: k,
		rows: open.filter((t) => (repo || reposOf(t)[0] || '') === k).sort((a, b) => RANK[groupOf(a, now)] - RANK[groupOf(b, now)] || newest(a, b)),
	}));
	return (
		<div class="t-home">
			<Filters hash={bare} repos={known} repo={repo} grp={filter.grp} who={whoseHome(hash)} />
			{filter.grp === 'repo' ? (
				stacks.map((s) =>
					s.rows.length ? <Section key={s.name} tone="repo" heading={s.name ? <RepoName repo={s.name} /> : 'No repo'} rows={s.rows} now={now} only={repo} everyone={everyone} peeking={peeking} /> : null,
				)
			) : (
				<>
					{repo && (
						<h2 class="t-repohead">
							<RepoName repo={repo} />
						</h2>
					)}
					<Section heading="Needs you" tone="needs" rows={by('needs')} now={now} only={repo} everyone={everyone} peeking={peeking} empty="Nothing needs you." />
					<Section heading="Working" tone="working" rows={by('working')} now={now} only={repo} everyone={everyone} peeking={peeking} empty="Nothing is running." />
					<Section heading="Done" tone="done" rows={by('done')} now={now} only={repo} everyone={everyone} peeking={peeking} empty="None." />
				</>
			)}
			<details class="t-settled">
				<summary>
					Settled
					<span class="t-cnt">
						<span class="t-sep"> · </span>
						{settled.length}
					</span>
				</summary>
				<div class="t-rows">
					{settled.map((t) => (
						<Row t={t} now={now} only={repo} everyone={everyone} key={t.box} settled peeking={t.box === peeking} />
					))}
				</div>
				<p class="muted t-settled-note">Closed, or done and quiet for 7 days.</p>
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
