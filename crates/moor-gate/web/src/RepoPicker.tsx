import { useState } from 'preact/hooks';

/** The repos a task clones, in order: the agent starts in the first, and the rest are cloned with it. */
export function RepoPicker({ repos, known, onChange }: { repos: string[]; known: string[]; onChange: (next: string[]) => void }) {
	const [typed, setTyped] = useState('');
	const add = () => {
		const repo = typed.trim();
		if (repo === '' || repos.includes(repo)) return setTyped('');
		onChange([...repos, repo]);
		setTyped('');
	};
	const remove = (i: number) => onChange(repos.filter((_, j) => j !== i));
	const first = (i: number) => onChange([repos[i], ...repos.filter((_, j) => j !== i)]);
	return (
		<div class="t-repos" role="group" aria-label="Repos, first one first">
			{repos.map((r, i) => (
				<span key={r} class={i === 0 ? 't-chip t-repo t-repo-first' : 't-chip t-repo'}>
					<span class="t-repo-n" aria-hidden="true">
						{i + 1}
					</span>
					<span class="t-mono">{r}</span>
					{i === 0 && repos.length > 1 && <span class="muted">starts here</span>}
					{i > 0 && (
						<button type="button" class="t-repo-btn" aria-label={`Start in ${r}`} title="Start here" onClick={() => first(i)}>
							↑
						</button>
					)}
					<button type="button" class="t-repo-btn" aria-label={`Remove ${r}`} onClick={() => remove(i)}>
						×
					</button>
				</span>
			))}
			<label class="t-chip">
				<span class="muted">{repos.length === 0 ? 'repo' : 'add repo'}</span>
				<input
					class="t-mono"
					list="t-repos"
					placeholder="owner/name"
					value={typed}
					onInput={(e) => setTyped(e.currentTarget.value)}
					onChange={add}
					onKeyDown={(e) => {
						if (e.key === 'Enter') {
							e.preventDefault();
							add();
						}
					}}
					onBlur={add}
					aria-label={repos.length === 0 ? 'Repo' : 'Add a repo'}
				/>
			</label>
			<datalist id="t-repos">
				{known
					.filter((r) => !repos.includes(r))
					.map((r) => (
						<option value={r} key={r} />
					))}
			</datalist>
		</div>
	);
}
