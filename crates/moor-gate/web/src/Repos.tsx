import { useEffect, useState } from 'preact/hooks';

import { Conflict, type RepoConfig, repoGet, repoList, repoSet, failure, toError } from './api';
import { GitHubApp } from './GitHubApp';
import { RepoRules } from './Rules';

const REPO = /^[\w.-]+\/[\w.-]+$/;

function when(iso: string): string {
	const at = new Date(iso);
	return Number.isNaN(at.getTime()) ? iso : at.toLocaleString();
}

function Picker({ repo }: { repo: string | null }) {
	const [repos, setRepos] = useState<string[]>([]);
	const [typed, setTyped] = useState(repo ?? '');
	useEffect(() => void repoList().then(setRepos, () => setRepos([])), []);
	useEffect(() => setTyped(repo ?? ''), [repo]);
	const open = (e: Event) => {
		e.preventDefault();
		location.hash = `#/repos/${typed.trim()}`;
	};
	return (
		<form onSubmit={open}>
			<div class="field">
				<input
					list="repos"
					aria-label="Repo"
					placeholder="owner/name"
					autocomplete="off"
					spellcheck={false}
					value={typed}
					onInput={(e) => setTyped(e.currentTarget.value)}
				/>
				<datalist id="repos">
					{repos.map((r) => <option key={r} value={r} />)}
				</datalist>
				<button disabled={!REPO.test(typed.trim())}>Open</button>
			</div>
		</form>
	);
}

function Help() {
	return (
		<>
			<p class="muted">
				One config for every Box of the repo, as YAML. A change reaches a running Box
				where the setting is read again.
			</p>
			<ul class="muted help">
				<li>
					Running Boxes: <code>env</code> on the next exec, run or setup line, <code>teardown</code> when the Box closes,{' '}
					<code>machine</code> when the agent next claims it, <code>compose</code> on the next command.
				</li>
				<li>
					Next Box: <code>setup</code>, <code>box.size</code>, <code>node.labels</code>, <code>cache</code> (a rebuilt Box
					counts as new). <code>brief</code> applies the next time the agent launches.
				</li>
			</ul>
		</>
	);
}

function Editor({ csrf, repo }: { csrf: string; repo: string }) {
	const [config, setConfig] = useState<RepoConfig | null>(null);
	const [draft, setDraft] = useState('');
	const [loadError, setLoadError] = useState('');
	const [busy, setBusy] = useState(false);
	const [problem, setProblem] = useState<{ text: string; stale: boolean } | null>(null);
	const [saved, setSaved] = useState(false);

	const load = async () => {
		setProblem(null);
		setSaved(false);
		setLoadError('');
		try {
			const read = await repoGet(repo);
			setConfig(read);
			setDraft(read.text ?? '');
		} catch (err) {
			setLoadError(failure(toError(err)));
		}
	};
	useEffect(() => {
		setConfig(null);
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
	if (!config) return <p class="muted" aria-busy="true">Loading {repo}…</p>;

	const save = async (e: Event) => {
		e.preventDefault();
		setBusy(true);
		setProblem(null);
		setSaved(false);
		try {
			const wrote = await repoSet(csrf, repo, draft, config.revision);
			setConfig(wrote);
			setDraft(wrote.text ?? draft);
			setSaved(true);
		} catch (err) {
			setProblem({ text: failure(toError(err)), stale: err instanceof Conflict });
		}
		setBusy(false);
	};
	const editable = config.can_edit;
	const dirty = draft !== (config.text ?? '');
	return (
		<form onSubmit={save}>
			<div class="row-head">
				<strong>{config.repo}</strong>
				<span class="pill">{config.revision === 0 ? 'No config yet' : `Revision ${config.revision}`}</span>
			</div>
			{config.edited_by && config.edited_at && (
				<p class="muted">Last edited by {config.edited_by} at {when(config.edited_at)}.</p>
			)}
			{!editable && (
				<p class="notice warn" role="status">
					Your GitHub account cannot push to this repo, so you can read its config here and not change it.
				</p>
			)}
			<textarea
				aria-label={`Config of ${config.repo}`}
				spellcheck={false}
				readOnly={!editable}
				rows={Math.min(30, Math.max(10, draft.split('\n').length + 1))}
				value={draft}
				onInput={(e) => setDraft(e.currentTarget.value)}
			/>
			{problem?.text && <pre class="notice danger" role="alert">{problem.text}</pre>}
			{saved && <p class="notice ok" role="status">Saved. The config is at revision {config.revision}.</p>}
			<div class="actions">
				{editable && <button disabled={busy || !dirty}>Save</button>}
				<button type="button" class="quiet" disabled={busy} onClick={load}>
					{problem?.stale ? 'Discard mine and load theirs' : 'Reload'}
				</button>
			</div>
		</form>
	);
}

export function Repos({ csrf, repo }: { csrf: string; repo: string | null }) {
	return (
		<>
			<GitHubApp />
			<section>
				<h2>Repo settings</h2>
				<Help />
				<Picker repo={repo} />
				{repo && REPO.test(repo) ? (
					<>
						<RepoRules key={repo} csrf={csrf} repo={repo} />
						<ul class="cards">
							<li><Editor key={repo} csrf={csrf} repo={repo} /></li>
						</ul>
					</>
				) : repo ? (
					<p class="notice danger" role="alert">Name a repo as owner/name.</p>
				) : null}
			</section>
		</>
	);
}
