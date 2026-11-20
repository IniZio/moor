import { workflowList, type WorkflowRow } from './api';
import { scheduleText } from './schedule';
import { RunNowButton } from './RunNow';
import { useLoad } from './useLoad';
import { Failed, PullTag, RepoSet, StatusIcon, when } from './WorkflowBits';

function Row({ w, csrf, onRan }: { w: WorkflowRow; csrf: string; onRan: () => void }) {
	return (
		<li>
			<div class="row-head">
				<a href={`#/workflows/${encodeURIComponent(w.name)}`}><strong>{w.name}</strong></a>
				<span class="pills">
					{w.plan_stale && <span class="pill warn">plan stale</span>}
					<span class={w.enabled ? 'pill ok' : 'pill'}>{w.enabled ? 'enabled' : 'disabled'}</span>
				</span>
			</div>
			<dl class="facts">
				<dt>Repos</dt>
				<dd>
					<RepoSet repos={w.repos} />
				</dd>
				<dt>Schedule</dt>
				<dd title={w.cron ?? undefined}>
					{scheduleText(w.words, w.cron, w.timezone)}
					{w.on_pull_request && (
						<>
							{' '}
							<PullTag />
						</>
					)}
				</dd>
				<dt>Last run</dt>
				<dd>
					{w.last ? (
						<>
							<StatusIcon status={w.last.status} /> {w.last.label}
							{w.last.status === 'failure' && (
								<>
									{' '}
									<RunNowButton csrf={csrf} name={w.name} onRan={onRan} hint=" after its failed last run" />
								</>
							)}
						</>
					) : (
						'none yet'
					)}
				</dd>
				{w.cron != null && (
					<>
						<dt>Next due</dt>
						<dd>{w.enabled ? (w.next_text ?? when(w.next_due)) : 'disabled'}</dd>
					</>
				)}
			</dl>
			{w.last?.status !== 'failure' && (
				<p class="run-now">
					<RunNowButton csrf={csrf} name={w.name} onRan={onRan} />
				</p>
			)}
		</li>
	);
}

export function List({ csrf }: { csrf: string }) {
	const { value, error, reload } = useLoad(workflowList, 'list');
	if (error) return <Failed error={error} retry={reload} />;
	if (!value) return <p class="muted" aria-busy="true">Loading workflows…</p>;
	return value.length === 0 ? (
		<p class="muted">No workflows yet. Make one with New workflow, or from a finished task with Make this recurring.</p>
	) : (
		<ul class="cards">{value.map((w) => <Row key={w.name} w={w} csrf={csrf} onRan={() => void reload()} />)}</ul>
	);
}
