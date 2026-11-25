import { useEffect, useState } from 'preact/hooks';

import { Conflict, type RepoConfig, type RulesSeen, repoGet, repoSet, failure, toError } from './api';
import './rules.css';

/** The line under the repo's name: who shares the rules, and which revision they are at. */
export function rulesLine(rules: RepoConfig): string {
	const shared = `Shared by everyone who works on ${rules.repo}`;
	if (rules.revision === 0) return `${shared} · the default rules`;
	const day = rules.edited_at ? new Date(rules.edited_at) : null;
	const on = day && !Number.isNaN(day.getTime()) ? ` ${day.toLocaleDateString(undefined, { day: 'numeric', month: 'short' })}` : '';
	const by = rules.edited_by ? ` · edited by ${rules.edited_by}${on}` : '';
	return `${shared} · revision ${rules.revision}${by}`;
}

/** The line numbers beside `text`, one per line; a closing newline starts no line of its own. */
export function lineNumbers(text: string): string {
	const lines = text.endsWith('\n') ? text.slice(0, -1).split('\n') : text.split('\n');
	return lines.map((_, i) => i + 1).join('\n');
}

/** A host as the lists show it: the port only when it is not 443. */
export function hostPort(host: string, port: number): string {
	return port === 443 || port === 0 ? host : `${host}:${port}`;
}

/** What the repo's Boxes met under the rules: requests sent on without the secret, and hosts reached without the Proxy. */
export function Seen({ seen }: { seen: RulesSeen }) {
	return (
		<aside class="rules-seen">
			<h3>Withheld in the last {seen.days} days</h3>
			{seen.withheld.length ? (
				<ul class="seen-rows">
					{seen.withheld.map((w) => (
						<li key={`${w.request} ${w.host}`}>
							<code>{w.request}</code>
							<code class="muted">{w.host}</code>
							<span class="seen-n" title="Boxes that reported it">{w.boxes}</span>
						</li>
					))}
				</ul>
			) : (
				<p class="muted">Nothing was withheld.</p>
			)}
			<p class="muted seen-note">A Box reports each request once, so the number counts Boxes.</p>
			<h3>Dialled directly</h3>
			{seen.dialled.length ? (
				<ul class="seen-rows">
					{seen.dialled.map((d) => (
						<li key={hostPort(d.host, d.port)}>
							<code>{hostPort(d.host, d.port)}</code>
							<span class="seen-n">{d.count}</span>
						</li>
					))}
				</ul>
			) : (
				<p class="muted">No host yet.</p>
			)}
			<p class="muted seen-note">Docker Boxes only. An address is a connection that skipped the Box's own proxy.</p>
		</aside>
	);
}

/** A repo's Proxy rules as their stored text, line by line, edited in place. */
export function RepoRules({ csrf, repo }: { csrf: string; repo: string }) {
	const [rules, setRules] = useState<RepoConfig | null>(null);
	const [draft, setDraft] = useState('');
	const [loadError, setLoadError] = useState('');
	const [busy, setBusy] = useState(false);
	const [problem, setProblem] = useState<{ text: string; stale: boolean } | null>(null);

	const load = async () => {
		setProblem(null);
		setLoadError('');
		try {
			const read = await repoGet(repo, 'rules');
			setRules(read);
			setDraft(read.text ?? '');
		} catch (err) {
			setLoadError(failure(toError(err)));
		}
	};
	useEffect(() => {
		setRules(null);
		void load();
	}, [repo]);

	if (loadError) {
		return (
			<>
				<p class="notice danger" role="alert">{loadError}</p>
				<button onClick={load}>Try again</button>
			</>
		);
	}
	if (!rules) return <p class="muted" aria-busy="true">Loading the rules of {repo}…</p>;

	const save = async (e: Event) => {
		e.preventDefault();
		setBusy(true);
		setProblem(null);
		try {
			const wrote = await repoSet(csrf, repo, draft, rules.revision, 'rules');
			setRules(wrote);
			setDraft(wrote.text ?? draft);
		} catch (err) {
			setProblem({ text: failure(toError(err)), stale: err instanceof Conflict });
		}
		setBusy(false);
	};
	const discard = () => {
		setProblem(null);
		setDraft(rules.text ?? '');
	};
	const editable = rules.can_edit;
	const dirty = draft !== (rules.text ?? '');
	const numbers = lineNumbers(draft);
	return (
		<section class="repo-rules">
			<h2>{rules.repo}</h2>
			<p class="rules-rev">{rulesLine(rules)}</p>
			{!editable && (
				<p class="notice warn" role="status">
					Your GitHub account cannot push to this repo, so you can read its rules here and not change them.
				</p>
			)}
			<div class={rules.seen ? 'rules-grid' : undefined}>
			<form class="rules-file" onSubmit={save}>
				<div class="rules-head">rules.yaml</div>
				<div class="rules-body">
					<pre class="rules-gutter" aria-hidden="true">{numbers}</pre>
					<textarea
						aria-label={`Rules of ${rules.repo}`}
						spellcheck={false}
						wrap="off"
						readOnly={!editable}
						rows={numbers.split('\n').length + 1}
						value={draft}
						onInput={(e) => setDraft(e.currentTarget.value)}
					/>
				</div>
				{problem?.text && <pre class="notice danger rules-problem" role="alert">{problem.text}</pre>}
				{editable && (dirty || problem?.stale) && (
					<div class="rules-foot">
						<button disabled={busy || !dirty}>Save</button>
						<button type="button" class="quiet" disabled={busy} onClick={problem?.stale ? load : discard}>
							{problem?.stale ? 'Discard mine and load theirs' : 'Discard'}
						</button>
					</div>
				)}
			</form>
			{rules.seen && <Seen seen={rules.seen} />}
			</div>
		</section>
	);
}
