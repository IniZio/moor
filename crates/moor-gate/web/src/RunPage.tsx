import { useState } from 'preact/hooks';

import { failure, type Line, type Run, runAnswer, runGet, type Step, toError } from './api';
import { RunNowButton } from './RunNow';
import { useLoad } from './useLoad';
import { duration, Failed, StatusIcon, when } from './WorkflowBits';

function LineItem({ line }: { line: Line }) {
	return (
		<li>
			<span class="muted">{when(line.at)}</span> {line.text}
			{line.evidence.length > 0 && (
				<ul class="evidence">
					{line.evidence.map((e) => (
						<li key={e.name}>
							{e.href ? <a href={e.href} download>{e.name}</a> : e.name}
						</li>
					))}
				</ul>
			)}
		</li>
	);
}

const STATE_WORD: Record<Step['state'], string> = {
	pending: 'not reached',
	running: 'running',
	waiting: 'waiting on you',
	passed: 'done',
	failed: 'failed',
	ended: 'ended',
};

function StepItem({ step, number }: { step: Step; number: number }) {
	return (
		<li>
			<details open={step.state === 'waiting' || step.state === 'failed'}>
				<summary>
					<span class={`kind ${step.kind}`}>{step.kind}</span>
					<span class="grow">{number}. {step.title}</span>
					<span class={`state ${step.state}`}>{STATE_WORD[step.state]}</span>
				</summary>
				{step.instructions && <p class="muted">{step.instructions}</p>}
				{step.command && <p class="muted">Runs <code>{step.command}</code></p>}
				{step.question && <p class="muted">Asks: {step.question}</p>}
				{step.done_when && <p class="muted">Done when: {step.done_when}</p>}
				{step.history.length > 0 ? (
					<ul class="history">{step.history.map((l) => <LineItem key={`${l.at}${l.kind}`} line={l} />)}</ul>
				) : (
					<p class="muted">This step was not reached.</p>
				)}
				{step.check && (
					<>
						<p class="muted">Check output, last 4 KiB, exit {step.check.exit ?? 'unknown'}</p>
						<pre aria-label={`Output of ${step.title}`}>{step.check.tail || '(no output)'}</pre>
					</>
				)}
			</details>
		</li>
	);
}

function Question({ run, csrf, onAnswered }: { run: Run; csrf: string; onAnswered: (run: Run) => void }) {
	const [busy, setBusy] = useState(false);
	const [problem, setProblem] = useState('');
	if (!run.question) return null;
	const answer = async (yes: boolean) => {
		setBusy(true);
		setProblem('');
		try {
			onAnswered(await runAnswer(csrf, run.workflow, run.id, yes));
		} catch (err) {
			setProblem(failure(toError(err)));
		}
		setBusy(false);
	};
	return (
		<div class="notice warn" role="group" aria-label="Question">
			<p><strong>{run.question.text || 'This run waits for your answer.'}</strong></p>
			<div class="actions">
				<button disabled={busy} onClick={() => void answer(true)}>Yes</button>
				<button disabled={busy} onClick={() => void answer(false)}>No</button>
			</div>
			{problem && <p class="notice danger" role="alert">{problem}</p>}
		</div>
	);
}

export function RunPage({ csrf, name, id }: { csrf: string; name: string; id: number }) {
	const { value: run, setValue, error, reload } = useLoad(() => runGet(name, id), `${name}/${id}`);
	return (
		<>
			<p><a href={`#/workflows/${encodeURIComponent(name)}`}>{name}</a></p>
			{error && <Failed error={error} retry={reload} />}
			{!run && !error && <p class="muted" aria-busy="true">Loading run…</p>}
			{run && (
				<>
					<div class="row-head">
						<h2 class="run-title">
							<StatusIcon status={run.status} /> Run {run.id}
						</h2>
						<span class="pill">{run.label}</span>
						{run.status === 'failure' && (
							<RunNowButton csrf={csrf} name={run.workflow} onRan={reload} hint=" after this failed run" />
						)}
						{run.outcome === 'skipped_fork' && run.pull && (
							<RunNowButton csrf={csrf} name={run.workflow} onRan={reload} pull={run.pull} label="Run again" hint={` for ${run.pull} at its current head`} />
						)}
					</div>
					<p class="muted">
						Due {when(run.due_at)}
						{run.duration_ms != null && `, took ${duration(run.duration_ms)}`}
						{run.box && `, Box ${run.box}`}
					</p>
					{run.result && <p class="notice ok">{run.result}</p>}
					{run.detail && <pre class="notice danger">{run.detail}</pre>}
					<Question run={run} csrf={csrf} onAnswered={setValue} />
					<ol class="steps">{run.steps.map((s, i) => <StepItem key={s.id} step={s} number={i + 1} />)}</ol>
					{run.lines.length > 0 && <ul class="history">{run.lines.map((l) => <LineItem key={`${l.at}${l.kind}`} line={l} />)}</ul>}
					<div class="actions"><button class="quiet" onClick={reload}>Refresh</button></div>
				</>
			)}
		</>
	);
}
