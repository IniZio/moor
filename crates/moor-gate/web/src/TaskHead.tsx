import type { ComponentChildren } from 'preact';

import type { BoxDetail } from './api';
import { STATES } from './Glyph';
import { exactTime, RepoName, reposOf } from './HomeList';

export type HeadFields = Pick<
	BoxDetail,
	'repo' | 'other_repos' | 'issue' | 'pr' | 'title' | 'state' | 'stale' | 'branch' | 'box' | 'machine' | 'workflow' | 'last_event_at' | 'updated_at' | 'line' | 'gate' | 'owner' | 'task_at'
>;

/** The number of a pull request named `OWNER/NAME#NUMBER`. */
const pullNumber = (pull: string) => pull.slice(pull.lastIndexOf('#') + 1);

const two = (n: number) => String(n).padStart(2, '0');
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/** The day and clock of `iso`, as a run's start reads: `24 Nov 09:00`. */
function dayTime(iso: string): string {
	const at = new Date(iso);
	return `${at.getDate()} ${MONTHS[at.getMonth()]} ${two(at.getHours())}:${two(at.getMinutes())}`;
}

/** The address of the pull request `OWNER/NAME#NUMBER` names. */
const pullUrl = (pull: string) => `https://github.com/${pull.slice(0, pull.lastIndexOf('#'))}/pull/${pullNumber(pull)}`;

/** A workflow run's line on the conversation page: the automation that started it, then the pull request it is for, or when it started. */
function RunMeta({ flow, pr, started }: { flow: NonNullable<HeadFields['workflow']>; pr: HeadFields['pr']; started?: string | null }) {
	const pull = flow.pull;
	return (
		<p class="t-th-meta t-th-run">
			started by <a href={`#/workflows/${encodeURIComponent(flow.name)}`}>{flow.name}</a>
			{pull ? (
				<>
					{' · pull request '}
					<a href={pr && String(pr.number) === pullNumber(pull) ? pr.url : pullUrl(pull)} target="_blank" rel="noopener">
						#{pullNumber(pull)}
					</a>
				</>
			) : (
				started && (
					<>
						{' · '}
						<time dateTime={started}>{dayTime(started)}</time>
					</>
				)
			)}
		</p>
	);
}

/** Which repos, issue and pull request the task is for, then its title and state, then a quiet line of where it runs and who started it when. The actions are the page's own and come in as `actions`; `pr` is where the pull request stands, after its link; `asked` is when the person wrote the prompt, from the timeline, or when a run started. `place` is the conversation page's head: the repos, the title and, for a run, what started it. */
export function TaskHead({ t, now, actions, pr, asked, place = false }: { t: HeadFields; now: number; actions: ComponentChildren; pr?: ComponentChildren; asked?: string | null; place?: boolean }) {
	const plain = place || !!t.task_at;
	const s = STATES[t.state];
	const repos = reposOf(t);
	const at = t.last_event_at ?? t.updated_at;
	const flow = t.workflow;
	const pull = flow?.pull ? pullNumber(flow.pull) : '';
	return (
		<header class="t-th">
			<div class="t-th0">
				{repos.map((r, i) => (
					<span class="t-th-repo" key={r}>
						{i > 0 && <span class="t-th-sep">·</span>}
						<RepoName repo={r} />
						{i === 0 && repos.length > 1 && <span class="t-th-starts">starts here</span>}
					</span>
				))}
				{t.issue && <span class="t-th-issue t-mono">{t.issue}</span>}
				{t.pr && !plain && (
					<a class="t-th-pr" href={t.pr.url} target="_blank" rel="noopener">
						PR #{t.pr.number}
					</a>
				)}
				{pr}
			</div>
			<div class="t-th1">
				<h1 class="t-th-title">{t.title}</h1>
				<span class={`t-pill t-${s.tone}${t.stale ? ' t-stale' : ''}`}>
					{s.word}
					{t.stale && ' · stale'}
				</span>
				<span class="grow" />
				{actions}
			</div>
			{plain && flow && <RunMeta flow={flow} pr={t.pr} started={asked} />}
			{!plain && (
				<p class="t-th-meta">
					<span class="t-mono">{t.branch ?? t.box}</span>
					{t.machine && <span>on {t.machine}</span>}
					{flow && (
						<span>
							started by <a href={`#/workflows/${encodeURIComponent(flow.name)}`}>{flow.name}</a>
							{pull && pull !== String(t.pr?.number) && ` · PR #${pull}`}
						</span>
					)}
					{!flow && asked ? (
						<span>
							asked by {t.owner ?? 'you'} at <time dateTime={asked}>{exactTime(asked, now)}</time>
						</span>
					) : (
						at && (
							<span>
								last event <time dateTime={at}>{exactTime(at, now)}</time>
							</span>
						)
					)}
				</p>
			)}
			{!plain && !t.gate?.gates.length && <p class="t-th-meta">{t.line}</p>}
		</header>
	);
}
