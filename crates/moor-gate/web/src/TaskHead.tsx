import type { ComponentChildren } from 'preact';

import type { BoxDetail } from './api';
import { STATES } from './Glyph';
import { exactTime, RepoName, reposOf } from './HomeList';

export type HeadFields = Pick<
	BoxDetail,
	'repo' | 'other_repos' | 'issue' | 'pr' | 'title' | 'state' | 'stale' | 'branch' | 'box' | 'machine' | 'workflow' | 'last_event_at' | 'updated_at' | 'line' | 'gate' | 'owner'
>;

/** The number of a pull request named `OWNER/NAME#NUMBER`. */
const pullNumber = (pull: string) => pull.slice(pull.lastIndexOf('#') + 1);

/** Which repos, issue and pull request the task is for, then its title and state, then a quiet line of where it runs and who started it when. The actions are the page's own and come in as `actions`; `pr` is where the pull request stands, after its link; `asked` is when the person wrote the prompt, from the timeline. */
export function TaskHead({ t, now, actions, pr, asked }: { t: HeadFields; now: number; actions: ComponentChildren; pr?: ComponentChildren; asked?: string | null }) {
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
				{t.pr && (
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
			{!t.gate?.gates.length && <p class="t-th-meta">{t.line}</p>}
		</header>
	);
}
