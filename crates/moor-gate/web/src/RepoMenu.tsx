import type { ComponentChildren } from 'preact';
import { useEffect, useState } from 'preact/hooks';

import type { BoxRow } from './api';
import { exactTime, RepoName } from './HomeList';
import { isRepoName, lastUsed, recentRepos, type Together, usedTogether } from './repoSets';

export const dropped = (repos: string[], repo: string): string[] => repos.filter((r) => r !== repo);
export const toggled = (repos: string[], repo: string): string[] => (repos.includes(repo) ? dropped(repos, repo) : [...repos, repo]);
export const startedIn = (repos: string[], repo: string): string[] => [repo, ...dropped(repos, repo)];

/** A repo in the checkbox list, with when it was last used. */
export type Listed = { repo: string; used: string };

export type Item = { kind: 'combo'; repos: string[]; count: number } | { kind: 'repo'; repo: string; used: string } | { kind: 'add'; repo: string };

/** The rows the arrow keys walk, in the order they are drawn: the filter narrows combinations and repos. */
export function menuItems(p: { repos: string[]; listed: Listed[]; together: Together[]; find: string }): Item[] {
	const q = p.find.trim().toLowerCase();
	const combos = p.together.filter((t) => t.repos.join(' ').toLowerCase().includes(q)).map((t): Item => ({ kind: 'combo', ...t }));
	const rows = p.listed.filter((l) => l.repo.toLowerCase().includes(q)).map((l): Item => ({ kind: 'repo', ...l }));
	const typed = p.find.trim();
	const add = isRepoName(typed) && !p.listed.some((l) => l.repo === typed) && !p.repos.includes(typed);
	return [...combos, ...rows, ...(add ? [{ kind: 'add' as const, repo: typed }] : [])];
}

export type MenuProps = {
	repos: string[];
	listed: Listed[];
	together: Together[];
	find: string;
	active: number;
	onFind: (text: string) => void;
	onActive: (at: number) => void;
	onChange: (repos: string[]) => void;
	onDone: () => void;
};

type KeyEvent = { key: string; preventDefault: () => void };

/** The repo menu without its hooks, so a test can read it. */
export function RepoMenuView(p: MenuProps) {
	const items = menuItems(p);
	const apply = (item: Item) => {
		if (item.kind === 'combo') p.onChange(item.repos);
		else if (item.kind === 'repo') p.onChange(toggled(p.repos, item.repo));
		else {
			p.onChange([...p.repos, item.repo]);
			p.onFind('');
		}
	};
	const typing = (e: KeyEvent) => {
		const down = e.key === 'ArrowDown';
		if (down || e.key === 'ArrowUp') {
			e.preventDefault();
			p.onActive(Math.max(0, Math.min(items.length - 1, p.active + (down ? 1 : -1))));
		} else if (e.key === ' ' && items[p.active]) {
			e.preventDefault();
			apply(items[p.active]);
		} else if (e.key === 'Enter') {
			e.preventDefault();
			const add = items.find((i) => i.kind === 'add');
			if (add) apply(add);
			else p.onDone();
		}
	};
	const onRow = (e: KeyEvent) => {
		if (e.key === 'Enter') {
			e.preventDefault();
			p.onDone();
		}
	};
	const at = (item: Item) => items.indexOf(item);
	const combos = items.filter((i) => i.kind === 'combo');
	const rows = items.filter((i) => i.kind === 'repo');
	const add = items.find((i) => i.kind === 'add');
	return (
		<div
			class="t-pm t-repomenu"
			role="dialog"
			aria-label="Repos for this task"
			onKeyDown={(e) => {
				if (e.key === 'Escape') {
					e.stopPropagation();
					p.onDone();
				}
			}}
		>
			<input
				id="t-rfind"
				class="t-pm-find"
				placeholder="Find a repo or type owner/name"
				aria-label="Find a repo or type owner/name"
				autocomplete="off"
				spellcheck={false}
				value={p.find}
				onInput={(e) => p.onFind(e.currentTarget.value)}
				onKeyDown={typing}
			/>
			{p.repos.length > 0 && (
				<>
					<span class="t-mhd">In this task</span>
					{p.repos.map((r, i) => (
						<span class="t-rch" key={r}>
							<span class="t-rname">
								<RepoName repo={r} />
							</span>
							{i === 0 ? (
								<span class="t-msub">starts here</span>
							) : (
								<button type="button" class="t-lnk" onClick={() => p.onChange(startedIn(p.repos, r))}>
									start here
								</button>
							)}
							<button type="button" class="t-rx" aria-label={`Drop ${r}`} title={`Drop ${r}`} onClick={() => p.onChange(dropped(p.repos, r))}>
								×
							</button>
						</span>
					))}
				</>
			)}
			{combos.length > 0 && (
				<>
					<span class="t-mhd">Used together before</span>
					{combos.map((c) => (
						<button type="button" class="t-rcombo" data-active={at(c) === p.active} key={c.repos.join(',')} onClick={() => apply(c)} onKeyDown={onRow}>
							<span class="t-mono">{c.repos.join(' + ')}</span>
							<span class="t-msub">
								{c.count} {c.count === 1 ? 'task' : 'tasks'}
							</span>
						</button>
					))}
				</>
			)}
			{rows.length > 0 && (
				<>
					<span class="t-mhd">Repos your GitHub access can see</span>
					{rows.map((r) => (
						<button
							type="button"
							class="t-rcheck"
							role="menuitemcheckbox"
							aria-checked={p.repos.includes(r.repo)}
							data-active={at(r) === p.active}
							key={r.repo}
							onClick={() => apply(r)}
							onKeyDown={onRow}
						>
							<span class="t-rbox" aria-hidden="true">
								{p.repos.includes(r.repo) ? '✓' : ''}
							</span>
							<span class="t-rname">
								<RepoName repo={r.repo} />
							</span>
							<span class="t-msub">{r.used && `used ${r.used}`}</span>
						</button>
					))}
				</>
			)}
			{add && (
				<button type="button" class="t-rcheck" data-active={at(add) === p.active} onClick={() => apply(add)} onKeyDown={onRow}>
					<span class="t-rbox" aria-hidden="true">
						+
					</span>
					<span>
						Add <span class="t-mono">{add.repo}</span>
					</span>
					<span class="t-msub">Enter</span>
				</button>
			)}
			{items.length === 0 && <span class="t-rnone muted">No repo you can see matches. Type a full owner/name to add one.</span>}
			<span class="t-pm-done">
				<span class="muted">Check several. The first is where the agent starts.</span>
				<button type="button" class="btn primary" onClick={p.onDone}>
					Done
				</button>
			</span>
		</div>
	);
}

/** Closes a menu on a click outside its `data-menu` wrapper; Escape inside it is the menu's own. */
export function useDismiss(name: string, onClose: () => void) {
	useEffect(() => {
		const on = (e: MouseEvent) => {
			if (!(e.target instanceof Element && e.target.closest(`[data-menu="${name}"]`))) onClose();
		};
		addEventListener('mousedown', on);
		return () => removeEventListener('mousedown', on);
	}, [name, onClose]);
}

/** Puts focus back on a menu's word once it closes by keyboard, so the keyboard stays where it was. */
export function refocus(name: string) {
	document.querySelector<HTMLElement>(`[data-menu="${name}"] > button`)?.focus();
}

/** The open menu: the filter owns focus, and what is checked changes as it is checked. */
export function RepoPanel({ repos, known, rows, onChange, onClose }: { repos: string[]; known: string[]; rows: BoxRow[]; onChange: (repos: string[]) => void; onClose: () => void }) {
	const [find, setFind] = useState('');
	const [active, setActive] = useState(-1);
	useDismiss('repo', onClose);
	useEffect(() => document.getElementById('t-rfind')?.focus(), []);
	useEffect(() => document.querySelector('[data-active="true"]')?.scrollIntoView?.({ block: 'nearest' }), [active]);
	const used = lastUsed(rows);
	const listed = recentRepos(rows, known).map((repo) => {
		const at = used.get(repo);
		return { repo, used: at ? exactTime(new Date(at).toISOString(), Date.now()) : '' };
	});
	return (
		<RepoMenuView
			repos={repos}
			listed={listed}
			together={usedTogether(rows)}
			find={find}
			active={active}
			onFind={(text) => {
				setFind(text);
				setActive(-1);
			}}
			onActive={setActive}
			onChange={onChange}
			onDone={() => {
				onClose();
				refocus('repo');
			}}
		/>
	);
}

/** The repo word of a sentence: the repos joined, and the menu under it while it is open. */
export function RepoSlot({ repos, open, onToggle, panel }: { repos: string[]; open: boolean; onToggle: () => void; panel: ComponentChildren }) {
	return (
		<span class="t-twwrap" data-menu="repo">
			<button type="button" class="t-tw t-mono" aria-haspopup="dialog" aria-expanded={open} onClick={onToggle}>
				{repos.length ? repos.join(' + ') : 'choose a repo'}
			</button>
			{panel}
		</span>
	);
}

/** A repo menu with its own open state, for a form that is not a sentence. */
export function RepoMenu({ repos, known, rows, onChange }: { repos: string[]; known: string[]; rows: BoxRow[]; onChange: (repos: string[]) => void }) {
	const [open, setOpen] = useState(false);
	return <RepoSlot repos={repos} open={open} onToggle={() => setOpen(!open)} panel={open && <RepoPanel repos={repos} known={known} rows={rows} onChange={onChange} onClose={() => setOpen(false)} />} />;
}
