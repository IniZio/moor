import { workflowList, type WorkflowRow } from './api';
import { RepoCell } from './HomeList';
import { rowTime } from './runGroups';
import { firesOn } from './schedule';
import { useLoad } from './useLoad';
import { Failed, markOf, StatusIcon } from './WorkflowBits';

/** One automation as a flat row: name, what fires it, its repos, its last run and when that was. Run now and switching it off sit in the automation's ⋯ menu. */
function Row({ w }: { w: WorkflowRow }) {
	const last = w.last;
	return (
		<a class={w.enabled ? 'arow' : 'arow off'} href={`#/workflows/${encodeURIComponent(w.name)}`}>
			<span class="aname">{w.name}</span>
			<span class="atrig" title={w.cron == null ? undefined : [w.cron, w.timezone ?? 'UTC'].join(' · ')}>
				{firesOn(w)}
				{w.cron != null && w.on_pull_request && ' and on every pull request'}
				{!w.enabled && ' · off'}
			</span>
			<RepoCell repos={w.repos} only="" />
			<span class="alast">
				{last ? (
					<>
						<StatusIcon status={last.status} verdict={last.verdict} />
						<span class={`outcome n-${markOf(last.status, last.verdict).look}`}>{last.label}</span>
					</>
				) : (
					<span class="outcome">no runs yet</span>
				)}
			</span>
			<span class="rtime">{last ? rowTime(last.due_at) : ''}</span>
		</a>
	);
}

export function List() {
	const { value, error, reload } = useLoad(workflowList, 'list');
	if (error) return <Failed error={error} retry={reload} />;
	if (!value) return <p class="muted" aria-busy="true">Loading automations…</p>;
	return value.length === 0 ? (
		<p class="muted">No automations yet. Make one with New automation, or from a finished task with Make this recurring.</p>
	) : (
		<div class="rows alist">
			{value.map((w) => (
				<Row key={w.name} w={w} />
			))}
		</div>
	);
}
